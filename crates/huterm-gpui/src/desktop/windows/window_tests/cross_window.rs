//! Private sessions make these transfers exercise viewer revocation.
use super::super::pane_drag::{DropTarget, SPRING_LOAD_DELAY};
use super::super::window_locator::{Located, WindowLocator};
use super::*;
use std::cell::Cell;

struct Locator(Cell<Located>);
impl WindowLocator for Locator {
    fn locate(
        &self,
        _: &Window,
        _: gpui::Point<Pixels>,
        _: &[(gpui::WindowId, u64)],
    ) -> Located {
        self.0.get()
    }
}
impl DesktopHarness {
    fn private_window(&mut self) -> (AnyWindowHandle, WorkspaceId, TabId) {
        let (session, workspace, tab, attachment) = {
            let mut mux = self.runtime.lock();
            let session = mux.create_session(None).unwrap();
            let workspace = mux.create_workspace(session, None).unwrap();
            let tab = mux.open_tab(workspace, &fixture(ECHO)).unwrap().tab.id;
            (
                session,
                workspace,
                tab,
                mux.attach_session(session).unwrap(),
            )
        };
        let handle = self.cx.update(|cx| {
            let before = cx.windows();
            open_window_inner(cx, false);
            let handle = cx
                .windows()
                .into_iter()
                .find(|handle| !before.contains(handle))
                .unwrap();
            handle
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>().unwrap().update(
                        cx,
                        |view, cx| {
                            view.adopt_attachment(
                                attachment, session, workspace, cx,
                            );
                        },
                    );
                })
                .unwrap();
            handle
        });
        self.wait_for("private window installation", |cx| {
            terminal(cx, handle, tab).is_some()
        });
        (handle, workspace, tab)
    }
    fn cross_locator(&mut self) -> Rc<Locator> {
        let locator = Rc::new(Locator(Cell::new(Located::Outside)));
        self.cx.update(|cx| {
            cx.global_mut::<Desktop>().cross.locator = locator.clone();
        });
        locator
    }
    fn cross_slot(&mut self, window: AnyWindowHandle) -> gpui::Point<Pixels> {
        self.update_view(window, |view, window, _| {
            view.tab_strip(window).label(0).origin + point(px(1.0), px(1.0))
        })
    }
    fn cross_begin(&mut self, source: AnyWindowHandle, tab: TabId) {
        self.update_view(source, |view, window, cx| {
            let index =
                view.tabs.iter().position(|view| view.id == tab).unwrap();
            let at = view.tab_strip(window).label(index).center();
            view.begin_reorder(tab, at, window, cx);
            view.update_reorder(at + point(px(8.0), px(8.0)), window, cx);
        });
        self.cx.run_until_parked();
    }
    fn cross_release(&mut self, source: AnyWindowHandle) {
        self.update_view(source, |view, window, cx| {
            view.finish_reorder(point(px(-100.0), px(-100.0)), window, cx);
        });
    }
    fn cross_idle(&mut self, destination: AnyWindowHandle, pane: PaneId) {
        self.wait_for("transfer installation and focus", |cx| {
            destination
                .update(cx, |root, _, cx| {
                    let view = root.downcast::<WorkspaceView>().unwrap();
                    let view = view.read(cx);
                    !view.busy
                        && view
                            .active_pane(cx)
                            .is_some_and(|(_, active)| active == pane)
                })
                .unwrap_or(false)
        });
    }
    fn cross_fixture(
        cx: &mut TestAppContext,
    ) -> (
        Self,
        AnyWindowHandle,
        TabId,
        AnyWindowHandle,
        WorkspaceId,
        TabId,
        Rc<Locator>,
    ) {
        let mut h =
            Self::with_config(cx, |config| config.tabs.always_show = true);
        let tab = h.open_tab(ECHO);
        let _ = h.open_tab(ECHO);
        let source = h.attach_window();
        let (destination, workspace, anchor) = h.private_window();
        h.activate(source, tab);
        let locator = h.cross_locator();
        let slot = h.cross_slot(destination);
        locator
            .0
            .set(Located::Window(destination.window_id(), slot));
        (h, source, tab, destination, workspace, anchor, locator)
    }
    fn cross_pane(&mut self, tab: TabId) -> PaneId {
        self.cx.update(|cx| {
            cx.global::<Desktop>()
                .hierarchy
                .state()
                .tab(tab)
                .unwrap()
                .layout
                .first_pane()
                .id
        })
    }
}

#[gpui::test]
fn cross_window_tabs_keep_ids_processes_and_slot_order(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, workspace, anchor, _locator) =
        DesktopHarness::cross_fixture(cx);
    let pane = h.cross_pane(tab);
    h.cross_begin(source, tab);
    assert!(h.update_view(destination, |view, _, _| {
        view.incoming.as_ref().is_some_and(|incoming| {
            incoming.target == Some(DropTarget::Slot(0))
        })
    }));
    h.cross_release(source);
    h.cross_idle(destination, pane);
    let mux = h.runtime.lock();
    assert_eq!(mux.tab(tab).unwrap().first_pane().id, pane);
    assert_eq!(
        mux.workspace(workspace)
            .unwrap()
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        [tab, anchor]
    );
    drop(mux);
    h.type_line(destination, "still-alive");
    h.wait_text(destination, tab, "got:still-alive");
}

#[gpui::test]
fn cross_window_cancel_rejects_late_preview_callbacks(cx: &mut TestAppContext) {
    let (mut h, source, tab, destination, workspace, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    h.cross_begin(source, tab);
    let late =
        h.cx.update(|cx| cx.global::<Desktop>().cross.drag.clone().unwrap());
    h.update_view(source, super::super::WorkspaceView::cancel_reorder);
    h.cx.run_until_parked();
    assert!(h.update_view(destination, |view, _, _| view.incoming.is_none()));
    // Deliver the captured callback only after the deferred clear has run.
    let pointer = h.cross_slot(destination);
    h.update_view(destination, |view, window, cx| {
        view.receive_incoming(late, pointer, window, cx);
    });
    assert!(h.update_view(destination, |view, _, _| view.incoming.is_none()));
    assert!(h.update_view(source, |view, _, _| view.reorder.is_none()
        && !view.busy));
    let mux = h.runtime.lock();
    assert!(contains(&mux, h.workspace, tab));
    assert!(!contains(&mux, workspace, tab));
}

#[gpui::test]
fn cross_window_release_resolves_again_and_refuses_busy_destinations(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, locator) =
        DesktopHarness::cross_fixture(cx);
    h.cross_begin(source, tab);
    locator.0.set(Located::Blocked);
    h.cross_release(source);
    h.cx.run_until_parked();
    assert!(contains(&h.runtime.lock(), h.workspace, tab));
    assert!(h.update_view(destination, |view, _, _| view.incoming.is_none()));
    let slot = h.cross_slot(destination);
    locator
        .0
        .set(Located::Window(destination.window_id(), slot));
    h.cross_begin(source, tab);
    h.update_view(destination, |view, _, _| view.busy = true);
    h.cross_release(source);
    h.cx.run_until_parked();
    assert!(contains(&h.runtime.lock(), h.workspace, tab));
}

#[gpui::test]
fn cross_window_reservations_precede_workers_and_close_requests(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.hold(Stage::Transfer);
    h.cross_release(source);
    let runtime = h.runtime.clone();
    h.wait_for("transfer worker to start", |_| {
        runtime.worker_hooks.held(Stage::Transfer) == 1
    });
    for handle in [source, destination] {
        assert!(h.update_view(handle, |view, _, _| view.busy));
        h.update_view(handle, |view, window, cx| {
            view.request_close(CloseTarget::Window, window, cx);
        });
    }
    h.cx.run_until_parked();
    assert_eq!(
        h.runtime.worker_hooks.entered(Stage::Assess),
        0,
        "closes must wait while the transfer worker is held"
    );
    h.finish_queued_cross_closes(source, destination);
}

#[gpui::test]
fn cross_window_failed_transfer_preserves_zoom(cx: &mut TestAppContext) {
    let (mut h, source, tab, destination, _, anchor, _locator) =
        DesktopHarness::cross_fixture(cx);
    let first = h.cross_pane(tab);
    let second = h.split(tab, first, SplitDirection::Right, ECHO);
    h.wait_for("source split installs", |cx| {
        pane_view(cx, source, tab, second).is_some()
    });
    h.invoke(source, ids::SWAP_PANE_NEXT).unwrap();
    h.wait_for("source layout history is recorded", |cx| {
        cx.global::<Desktop>()
            .windows
            .record(source.window_id())
            .unwrap()
            .layout_history
            .peek(huterm_core::RestoreDirection::Undo)
            .is_some()
    });
    let history = h.cx.update(|cx| {
        cx.global::<Desktop>()
            .windows
            .record(source.window_id())
            .unwrap()
            .layout_history
            .clone()
    });
    let pane = h.cross_pane(tab);
    let other = h.cross_pane(anchor);
    h.update_view(source, |view, _, _| {
        view.tabs
            .iter_mut()
            .find(|view| view.id == tab)
            .unwrap()
            .zoomed = Some(pane);
    });
    h.update_view(destination, |view, _, _| {
        view.tabs
            .iter_mut()
            .find(|view| view.id == anchor)
            .unwrap()
            .zoomed = Some(other);
    });
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.fail(Stage::Transfer);
    h.cross_release(source);
    h.wait_for("failed transfer completion", |cx| {
        source
            .update(cx, |root, _, cx| {
                !root.downcast::<WorkspaceView>().unwrap().read(cx).busy
            })
            .unwrap()
    });
    assert_eq!(
        h.update_view(source, |view, _, _| view
            .tabs
            .iter()
            .find(|view| view.id == tab)
            .unwrap()
            .zoomed),
        Some(pane)
    );
    assert_eq!(
        h.update_view(destination, |view, _, _| view
            .tabs
            .iter()
            .find(|view| view.id == anchor)
            .unwrap()
            .zoomed),
        Some(other)
    );
    assert_eq!(
        h.cx.update(|cx| cx
            .global::<Desktop>()
            .windows
            .record(source.window_id())
            .unwrap()
            .layout_history
            .clone()),
        history
    );
    assert!(contains(&h.runtime.lock(), h.workspace, tab));
}

#[gpui::test]
fn cross_window_spring_load_and_back_keep_source_focus(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, workspace, origin, locator) =
        DesktopHarness::cross_fixture(cx);
    let sprung = h
        .runtime
        .lock()
        .open_tab(workspace, &fixture(ECHO))
        .unwrap()
        .tab
        .id;
    h.wait_for("second destination tab", |cx| {
        terminal(cx, destination, sprung).is_some()
    });
    let label = h.update_view(destination, |view, window, _| {
        view.tab_strip(window).label(1).center()
    });
    locator
        .0
        .set(Located::Window(destination.window_id(), label));
    h.cross_begin(source, tab);
    h.update_view(destination, |view, window, cx| {
        view.advance_incoming_spring(
            Instant::now() + SPRING_LOAD_DELAY,
            window,
            cx,
        );
    });
    assert_eq!(
        h.update_view(destination, |view, _, cx| view.active_tab(cx)),
        Some(sprung)
    );
    assert!(h.cx.update(|cx| focused(cx, source, tab)));
    h.update_view(source, super::super::WorkspaceView::cancel_reorder);
    h.cx.run_until_parked();
    assert_eq!(
        h.update_view(destination, |view, _, cx| view.active_tab(cx)),
        Some(origin)
    );
    assert!(h.cx.update(|cx| focused(cx, source, tab)));
}

#[gpui::test]
fn cross_window_tear_off_handles_creation_failure_before_moving(
    cx: &mut TestAppContext,
) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
    });
    let tab = h.open_tab(ECHO);
    let _ = h.open_tab(ECHO);
    let source = h.attach_window();
    h.activate(source, tab);
    let pane = h.cross_pane(tab);
    let _locator = h.cross_locator();
    h.cx.update(|cx| cx.global_mut::<Desktop>().cross.fail_window = true);
    h.cross_begin(source, tab);
    h.cross_release(source);
    h.wait_for("failed window creation", |cx| {
        source
            .update(cx, |root, _, cx| {
                !root.downcast::<WorkspaceView>().unwrap().read(cx).busy
            })
            .unwrap()
    });
    assert_eq!(h.cx.update(|cx| cx.windows().len()), 1);
    assert!(contains(&h.runtime.lock(), h.workspace, tab));
    h.cross_begin(source, tab);
    h.cross_release(source);
    let mut destination = None;
    h.wait_for("tear-off window", |cx| {
        destination = cx.windows().into_iter().find(|handle| *handle != source);
        destination.is_some()
    });
    h.cross_idle(destination.unwrap(), pane);
}

#[gpui::test]
fn cross_window_lone_tab_tear_off_does_nothing(cx: &mut TestAppContext) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
    });
    let tab = h.open_tab(ECHO);
    let source = h.attach_window();
    h.activate(source, tab);
    let _locator = h.cross_locator();
    h.cross_begin(source, tab);
    assert!(!h.update_view(source, |view, _, cx| view.tear_off_cue(cx)));
    h.cross_release(source);
    h.cx.run_until_parked();
    assert_eq!(h.cx.update(|cx| cx.windows().len()), 1);
    assert!(contains(&h.runtime.lock(), h.workspace, tab));
}

fn contains(mux: &Mux, workspace: WorkspaceId, tab: TabId) -> bool {
    mux.workspace(workspace).is_some_and(|workspace| {
        workspace.tabs.iter().any(|entry| entry.id == tab)
    })
}

impl DesktopHarness {
    fn cross_begin_pane(
        &mut self,
        source: AnyWindowHandle,
        tab: TabId,
        pane: PaneId,
    ) {
        self.update_view(source, |view, window, cx| {
            let at = view.drawn_frame(pane).unwrap().center();
            view.begin_pane_drag(tab, pane, at, false, window, cx);
            view.update_pane_drag(at + point(px(8.0), px(8.0)), window, cx);
        });
        self.cx.run_until_parked();
    }
    fn cross_release_pane(&mut self, source: AnyWindowHandle) {
        self.update_view(source, |view, window, cx| {
            view.finish_pane_drag(point(px(-100.0), px(-100.0)), window, cx);
        });
    }
}

#[gpui::test]
fn cross_window_lone_pane_moves_to_another_windows_slot(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, workspace, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    let pane = h.cross_pane(tab);
    h.cross_begin_pane(source, tab, pane);
    h.cross_release_pane(source);
    h.cross_idle(destination, pane);
    assert!(contains(&h.runtime.lock(), workspace, tab));
    assert!(!contains(&h.runtime.lock(), h.workspace, tab));
}

#[gpui::test]
fn cross_window_panes_refuse_centres_then_nest_at_edges(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, anchor, locator) =
        DesktopHarness::cross_fixture(cx);
    let pane = h.cross_pane(tab);
    let target = h.cross_pane(anchor);
    let frame = h.update_view(destination, |view, _, _| {
        view.drawn_frame(target).unwrap()
    });
    locator
        .0
        .set(Located::Window(destination.window_id(), frame.center()));
    h.cross_begin_pane(source, tab, pane);
    assert!(h.update_view(destination, |view, _, _| {
        view.incoming.as_ref().unwrap().target.is_none()
    }));
    h.cross_release_pane(source);
    h.cx.run_until_parked();
    assert!(contains(&h.runtime.lock(), h.workspace, tab));
    let edge = point(frame.left() + px(3.0), frame.center().y);
    locator
        .0
        .set(Located::Window(destination.window_id(), edge));
    h.cross_begin_pane(source, tab, pane);
    h.cross_release_pane(source);
    h.cross_idle(destination, pane);
    assert!(
        h.runtime
            .lock()
            .tab(anchor)
            .unwrap()
            .layout()
            .pane(pane)
            .is_some()
    );
    assert!(h.runtime.lock().tab(tab).is_none());
}

#[gpui::test]
fn cross_window_tab_nests_at_destination_pane_edge(cx: &mut TestAppContext) {
    let (mut h, source, tab, destination, _, anchor, locator) =
        DesktopHarness::cross_fixture(cx);
    let pane = h.cross_pane(tab);
    let target = h.cross_pane(anchor);
    let frame = h.update_view(destination, |view, _, _| {
        view.drawn_frame(target).unwrap()
    });
    locator.0.set(Located::Window(
        destination.window_id(),
        point(frame.right() - px(3.0), frame.center().y),
    ));
    h.cross_begin(source, tab);
    h.cross_release(source);
    h.cross_idle(destination, pane);
    assert!(h.runtime.lock().tab(tab).is_none());
    assert!(
        h.runtime
            .lock()
            .tab(anchor)
            .unwrap()
            .layout()
            .pane(pane)
            .is_some()
    );
}

#[gpui::test]
fn cross_window_last_tab_source_closes_before_destination_installs(
    cx: &mut TestAppContext,
) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
    });
    let tab = h.open_tab(ECHO);
    let source = h.attach_window();
    let (destination, workspace, _) = h.private_window();
    h.activate(source, tab);
    let pane = h.cross_pane(tab);
    let locator = h.cross_locator();
    let slot = h.cross_slot(destination);
    locator
        .0
        .set(Located::Window(destination.window_id(), slot));
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.hold(Stage::Subscribe);
    h.cross_release(source);
    let runtime = h.runtime.clone();
    h.wait_for("source closes while destination installation waits", |cx| {
        !cx.windows().contains(&source)
            && runtime.worker_hooks.held(Stage::Subscribe) == 1
    });
    assert!(contains(&h.runtime.lock(), workspace, tab));
    assert!(h.update_view(destination, |view, _, _| view.busy
        && view.transfer.is_some()));
    h.runtime.worker_hooks.release(Stage::Subscribe);
    h.cross_idle(destination, pane);
    h.type_line(destination, "survived");
    h.wait_text(destination, tab, "got:survived");
}

#[gpui::test]
fn cross_window_title_row_accepts_a_lone_pane_only_when_the_bar_is_hidden(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, locator) =
        DesktopHarness::cross_fixture(cx);
    let pane = h.cross_pane(tab);
    h.update_view(destination, |view, window, cx| {
        view.config.tabs.always_show = false;
        view.config.tabs.position = TabPosition::Titlebar;
        if cfg!(not(target_os = "macos")) {
            view.decorations_override = Some(gpui::Decorations::Client {
                tiling: gpui::Tiling::default(),
            });
        }
        view.sync_frame(window);
        view.sync_tab_layout(window, cx);
        cx.notify();
    });
    h.cx.run_until_parked();
    let row = h.update_view(destination, |view, window, _| {
        view.title_row_layout(&view.chrome_layout(window))
            .drop_region
    });
    locator
        .0
        .set(Located::Window(destination.window_id(), row.center()));
    h.cross_begin_pane(source, tab, pane);
    assert!(h.update_view(destination, |view, _, _| {
        view.incoming.as_ref().unwrap().target == Some(DropTarget::TitleRow)
    }));
    h.cross_release_pane(source);
    h.cross_idle(destination, pane);
}

#[gpui::test]
fn cross_window_command_and_failed_tear_off_share_the_transfer_path(
    cx: &mut TestAppContext,
) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
    });
    let tab = h.open_tab(ECHO);
    let _ = h.open_tab(ECHO);
    let source = h.attach_window();
    h.activate(source, tab);
    let pane = h.cross_pane(tab);
    let sessions: Vec<_> = h
        .runtime
        .lock()
        .sessions()
        .iter()
        .map(|session| session.id)
        .collect();
    h.runtime.worker_hooks.fail(Stage::Transfer);
    h.invoke(source, ids::MOVE_TAB_TO_NEW_WINDOW).unwrap();
    h.wait_for("failed command closes its empty destination", |cx| {
        cx.windows().len() == 1
            && source
                .update(cx, |root, _, cx| {
                    !root.downcast::<WorkspaceView>().unwrap().read(cx).busy
                })
                .unwrap()
    });
    assert!(contains(&h.runtime.lock(), h.workspace, tab));
    assert_eq!(
        h.runtime
            .lock()
            .sessions()
            .iter()
            .map(|session| session.id)
            .collect::<Vec<_>>(),
        sessions,
        "failed destination session must be deleted"
    );
    assert!(h.cx.update(|cx| {
        notices(cx, source)
            .iter()
            .any(|notice| notice.contains("Cannot move terminal"))
    }));
    h.invoke(source, ids::MOVE_TAB_TO_NEW_WINDOW).unwrap();
    let mut destination = None;
    h.wait_for("command opens the destination", |cx| {
        destination = cx.windows().into_iter().find(|handle| *handle != source);
        destination.is_some()
    });
    h.cross_idle(destination.unwrap(), pane);
}

#[gpui::test]
fn cross_window_destination_closure_cancels_the_source_gesture(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    h.cross_begin(source, tab);
    h.remove_window(destination);
    h.cx.run_until_parked();
    assert!(h.update_view(source, |view, _, _| view.reorder.is_none()));
    assert!(contains(&h.runtime.lock(), h.workspace, tab));
}

fn cross_window_placement(cx: &mut TestAppContext, position: TabPosition) {
    let (mut h, source, tab, destination, workspace, _, locator) =
        DesktopHarness::cross_fixture(cx);
    h.update_view(destination, |view, window, cx| {
        view.config.tabs.position = position;
        view.sync_tab_layout(window, cx);
        cx.notify();
    });
    h.cx.run_until_parked();
    let slot = h.cross_slot(destination);
    locator
        .0
        .set(Located::Window(destination.window_id(), slot));
    let pane = h.cross_pane(tab);
    h.cross_begin(source, tab);
    h.cross_release(source);
    h.cross_idle(destination, pane);
    assert_eq!(
        h.runtime.lock().workspace(workspace).unwrap().tabs[0].id,
        tab
    );
}
#[gpui::test]
fn cross_window_bottom_bar_slot(cx: &mut TestAppContext) {
    cross_window_placement(cx, TabPosition::Bottom);
}
#[gpui::test]
fn cross_window_left_bar_slot(cx: &mut TestAppContext) {
    cross_window_placement(cx, TabPosition::Left);
}
#[gpui::test]
fn cross_window_right_bar_slot(cx: &mut TestAppContext) {
    cross_window_placement(cx, TabPosition::Right);
}

#[gpui::test]
fn cross_window_overflow_autoscrolls_under_a_stationary_pointer(
    cx: &mut TestAppContext,
) {
    use super::super::refresh::Animated;
    let (mut h, source, tab, destination, workspace, _, locator) =
        DesktopHarness::cross_fixture(cx);
    for _ in 0..16 {
        h.runtime
            .lock()
            .open_tab(workspace, &fixture(ECHO))
            .unwrap();
    }
    h.wait_for("overflowed destination tabs", |cx| {
        destination
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .unwrap()
                    .read(cx)
                    .tabs
                    .len()
                    == 17
            })
            .unwrap()
    });
    let edge = h.update_view(destination, |view, window, cx| {
        view.tab_scroll = px(60.0);
        view.scroll_target = None;
        cx.notify();
        let strip = view.tab_strip(window);
        point(strip.bounds.right() - px(1.0), strip.bounds.center().y)
    });
    h.cx.run_until_parked();
    locator
        .0
        .set(Located::Window(destination.window_id(), edge));
    h.cross_begin(source, tab);
    let before = h.update_view(destination, |view, _, _| view.tab_scroll);
    h.update_view(destination, |view, window, cx| {
        view.advance_animation(
            Instant::now() + Duration::from_millis(80),
            true,
            window,
            cx,
        );
    });
    assert!(h.update_view(destination, |view, _, _| view.tab_scroll > before));
    let slot = h.update_view(destination, |view, window, _| {
        let strip = view.tab_strip(window);
        let index = (0..strip.count)
            .find(|index| strip.start(*index) >= strip.offset)
            .unwrap();
        point(
            strip.bounds.left() + strip.start(index) - strip.offset + px(1.0),
            strip.bounds.center().y,
        )
    });
    locator
        .0
        .set(Located::Window(destination.window_id(), slot));
    let pane = h.cross_pane(tab);
    h.cross_release(source);
    h.cross_idle(destination, pane);
}

#[gpui::test]
fn cross_window_install_failure_settles_without_an_outside_mutation(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, workspace, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    let pane = h.cross_pane(tab);
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.fail(Stage::Subscribe);
    h.cross_release(source);
    h.wait_for("failed install notice", |cx| {
        notices(cx, destination).iter().any(|notice| {
            notice.contains("injected viewer subscription failure")
        })
    });
    assert!(
        h.update_view(destination, |view, _, _| !view.busy
            && view.transfer.is_none()),
        "failed install must settle the reservation"
    );
    assert!(contains(&h.runtime.lock(), workspace, tab));
    h.update_view(destination, |view, window, cx| {
        let id = view.notices.newest().unwrap();
        let notice = &view.notices.get(id).unwrap().content;
        assert_eq!(notice.title, "Cannot show moved tab");
        assert_eq!(notice.actions[0].label, "Try Again");
        view.run_notice_action(id, None, window, cx).unwrap();
    });
    h.wait_for("retry installs moved pane", |cx| {
        pane_view(cx, destination, tab, pane).is_some()
    });
    h.update_view(destination, |view, window, cx| {
        view.select_tab(tab, true, window, cx);
    });
    h.type_line(destination, "retry-alive");
    h.wait_text(destination, tab, "got:retry-alive");
}

#[gpui::test]
fn cross_window_pending_pane_close_follows_the_moved_pane(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    let pane = h.cross_pane(tab);
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.hold(Stage::Transfer);
    h.cross_release(source);
    let runtime = h.runtime.clone();
    h.wait_for("transfer start", |_| {
        runtime.worker_hooks.held(Stage::Transfer) == 1
    });
    h.update_view(source, |view, window, cx| {
        view.request_close(CloseTarget::Pane { tab, pane }, window, cx);
    });
    h.runtime.worker_hooks.hold(Stage::Assess);
    h.runtime.worker_hooks.release(Stage::Transfer);
    h.wait_for("destination takes the pending pane close", |_| {
        runtime.worker_hooks.held(Stage::Assess) == 1
    });
    assert!(h.update_view(destination, |view, _, _| matches!(view.close.current, Some(CloseTarget::Pane { pane: closing, .. }) if closing == pane)));
    assert!(
        h.update_view(source, |view, _, _| view.close.current.is_none()
            && view.close.pending.is_none())
    );
    h.runtime.worker_hooks.release(Stage::Assess);
}

#[gpui::test]
fn cross_window_input_sync_and_focused_pane_follow_a_tab(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    let first = h.cross_pane(tab);
    let second = h.split(tab, first, SplitDirection::Right, ECHO);
    h.wait_for("source pane installed", |cx| {
        pane_view(cx, source, tab, second).is_some()
    });
    h.update_view(source, |view, window, cx| {
        view.focus_pane(tab, second, window, cx);
    });
    h.runtime
        .lock()
        .set_input_sync(h.workspace, tab, true)
        .unwrap();
    h.wait_for("input sync projection", |cx| {
        input_sync::syncs_input(tab, cx)
    });
    let terminals = h.runtime.lock().tab(tab).unwrap().terminals();
    h.cross_begin(source, tab);
    h.cross_release(source);
    h.cross_idle(destination, second);
    assert!(h.runtime.lock().tab(tab).unwrap().input_sync());
    assert_eq!(h.runtime.lock().tab(tab).unwrap().terminals(), terminals);
    h.type_line(destination, "synced");
    h.wait_for("both moved panes receive synced input", |cx| {
        [first, second].into_iter().all(|pane| {
            pane_screen(cx, destination, tab, pane)
                .is_some_and(|text| text.contains("got:synced"))
        })
    });
    h.cx.update(|cx| {
        for record in cx.global::<Desktop>().windows.records() {
            assert!(
                record
                    .layout_history
                    .peek(huterm_core::RestoreDirection::Undo)
                    .is_none()
            );
        }
    });
}

#[gpui::test]
fn cross_window_exited_tab_keeps_retained_output(cx: &mut TestAppContext) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.terminal.close_on_exit = false;
    });
    let tab = h.open_tab("printf RETAINED; exit 7");
    let _ = h.open_tab(ECHO);
    let source = h.attach_window();
    h.wait_for("source terminal exits", |cx| exited(cx, source, tab));
    let (destination, _, _) = h.private_window();
    h.activate(source, tab);
    let pane = h.cross_pane(tab);
    let locator = h.cross_locator();
    let slot = h.cross_slot(destination);
    locator
        .0
        .set(Located::Window(destination.window_id(), slot));
    h.cross_begin(source, tab);
    h.cross_release(source);
    h.cross_idle(destination, pane);
    h.wait_text(destination, tab, "RETAINED");
    h.wait_for("destination retains exited state", |cx| {
        exited(cx, destination, tab)
    });
}

#[gpui::test]
fn cross_window_activation_waits_for_every_moved_view(cx: &mut TestAppContext) {
    let (mut h, source, tab, destination, _, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    let pane = h.cross_pane(tab);
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.hold(Stage::Subscribe);
    h.cross_release(source);
    let runtime = h.runtime.clone();
    h.wait_for("destination install worker", |_| {
        runtime.worker_hooks.held(Stage::Subscribe) == 1
    });
    assert!(h.cx.update(|cx| {
        source
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap()
    }));
    assert!(!h.cx.update(|cx| {
        destination
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap()
    }));
    h.runtime.worker_hooks.release(Stage::Subscribe);
    h.cross_idle(destination, pane);
    h.wait_for("destination activated after install", |cx| {
        focused(cx, destination, tab)
    });
}

#[gpui::test]
fn cross_window_failed_drop_restores_destination_spring_preview(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, workspace, origin, locator) =
        DesktopHarness::cross_fixture(cx);
    let sprung = h
        .runtime
        .lock()
        .open_tab(workspace, &fixture(ECHO))
        .unwrap()
        .tab
        .id;
    h.wait_for("second destination tab", |cx| {
        terminal(cx, destination, sprung).is_some()
    });
    let label = h.update_view(destination, |view, window, _| {
        view.tab_strip(window).label(1).center()
    });
    locator
        .0
        .set(Located::Window(destination.window_id(), label));
    h.cross_begin(source, tab);
    h.update_view(destination, |view, window, cx| {
        view.advance_incoming_spring(
            Instant::now() + SPRING_LOAD_DELAY,
            window,
            cx,
        );
    });
    let target = h.cross_pane(sprung);
    h.wait_for("sprung tab draws", |cx| {
        destination
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .unwrap()
                    .read(cx)
                    .drawn_frame(target)
                    .is_some()
            })
            .unwrap()
    });
    let edge = h.update_view(destination, |view, _, _| {
        let frame = view.drawn_frame(target).unwrap();
        point(frame.right() - px(3.0), frame.center().y)
    });
    locator
        .0
        .set(Located::Window(destination.window_id(), edge));
    h.runtime.worker_hooks.fail(Stage::Transfer);
    h.cross_release(source);
    h.wait_for("failed drop springs back", |cx| {
        destination
            .update(cx, |root, _, cx| {
                let view = root.downcast::<WorkspaceView>().unwrap();
                !view.read(cx).busy
                    && view.read(cx).active_tab(cx) == Some(origin)
            })
            .unwrap()
    });
    assert!(h.cx.update(|cx| focused(cx, source, tab)));
}

#[gpui::test]
fn cross_window_blur_and_opening_a_menu_cancel_incoming_cues(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    h.cross_begin(source, tab);
    h.deactivate(source);
    h.cx.run_until_parked();
    assert!(h.update_view(source, |view, _, _| view.reorder.is_none()));
    assert!(h.update_view(destination, |view, _, _| view.incoming.is_none()));
    h.activate(source, tab);
    h.cross_begin(source, tab);
    h.invoke(source, ids::OPEN_MENU).unwrap();
    h.cx.run_until_parked();
    assert!(h.update_view(source, |view, _, _| view.reorder.is_none()));
    assert!(h.update_view(destination, |view, _, _| view.incoming.is_none()));
    assert!(contains(&h.runtime.lock(), h.workspace, tab));
}

#[gpui::test]
fn cross_window_exit_close_follows_a_pane_that_exits_during_transfer(
    cx: &mut TestAppContext,
) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.terminal.close_on_exit = true;
    });
    let tab = h.open_tab(EXITS);
    let _ = h.open_tab(ECHO);
    let source = h.attach_window();
    let (destination, _, _) = h.private_window();
    h.activate(source, tab);
    h.wait_text(source, tab, "READY");
    let locator = h.cross_locator();
    let slot = h.cross_slot(destination);
    locator
        .0
        .set(Located::Window(destination.window_id(), slot));
    let pane = h.cross_pane(tab);
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.hold(Stage::Transfer);
    h.cross_release(source);
    let runtime = h.runtime.clone();
    h.wait_for("transfer worker starts", |_| {
        runtime.worker_hooks.held(Stage::Transfer) == 1
    });
    let viewer = h.send_line(tab, "exit");
    h.wait_for("source observes the exit", |cx| exited(cx, source, tab));
    h.runtime.worker_hooks.release(Stage::Transfer);
    h.wait_for("moved exit-close settles", |cx| {
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .pane_tab(pane)
            .is_none()
    });
    assert!(h.runtime.lock().tab(tab).is_none());
    assert_eq!(h.close_failures(), 0);
    drop(viewer);
}

#[gpui::test]
fn cross_window_incoming_drag_reveals_an_inactive_fullscreen_bar(
    cx: &mut TestAppContext,
) {
    use super::super::refresh::Animated;
    let (mut h, source, tab, destination, _, _, locator) =
        DesktopHarness::cross_fixture(cx);
    h.update_view(destination, |view, window, cx| {
        window.toggle_fullscreen();
        view.fullscreen.sample(true, window.window_bounds());
        view.fullscreen.observe_native_flag(true);
        view.config.tabs.auto_hide_in_fullscreen = true;
        view.sync_tab_layout(window, cx);
        cx.notify();
    });
    h.cx.run_until_parked();
    locator.0.set(Located::Window(
        destination.window_id(),
        point(px(100.0), px(1.0)),
    ));
    h.cross_begin(source, tab);
    h.update_view(destination, |view, window, cx| {
        view.advance_animation(
            Instant::now() + Duration::from_millis(200),
            true,
            window,
            cx,
        );
    });
    assert!(h.update_view(destination, |view, _, _| view.bar_drawn()));
    assert!(!h.cx.update(|cx| {
        destination
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap()
    }));
    assert!(h.cx.update(|cx| focused(cx, source, tab)));
}

#[gpui::test]
fn cross_window_foreign_runtime_is_refused_with_a_notice(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, workspace, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    h.cross_begin(source, tab);
    h.update_view(destination, |view, _, cx| {
        let attachment = view.attachment_id(cx);
        let session = view.attached_session(cx).unwrap();
        cx.global_mut::<Desktop>().windows.attach(
            view.window,
            attachment,
            session,
            WorkspaceId::in_runtime(RuntimeId::new(u64::MAX), 1),
        );
    });
    h.cross_release(source);
    h.cx.run_until_parked();
    assert!(contains(&h.runtime.lock(), h.workspace, tab));
    assert!(!contains(&h.runtime.lock(), workspace, tab));
    assert!(h.cx.update(|cx| {
        notices(cx, source)
            .iter()
            .any(|notice| notice.contains("different runtimes"))
    }));
}

#[gpui::test]
fn cross_window_destination_close_then_source_close_wait_for_the_transfer(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.hold(Stage::Transfer);
    h.cross_release(source);
    let runtime = h.runtime.clone();
    h.wait_for("transfer starts", |_| {
        runtime.worker_hooks.held(Stage::Transfer) == 1
    });
    for handle in [destination, source] {
        h.update_view(handle, |view, window, cx| {
            view.request_close(CloseTarget::Window, window, cx);
        });
    }
    h.cx.run_until_parked();
    assert_eq!(runtime.worker_hooks.entered(Stage::Assess), 0);
    h.finish_queued_cross_closes(source, destination);
}

#[gpui::test]
fn cross_window_release_refreshes_membership_after_a_source_pane_closes(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, _locator) =
        DesktopHarness::cross_fixture(cx);
    let first = h.cross_pane(tab);
    let second = h.split(tab, first, SplitDirection::Right, ECHO);
    h.wait_for("second source pane installs", |cx| {
        pane_view(cx, source, tab, second).is_some()
    });
    h.update_view(source, |view, window, cx| {
        view.focus_pane(tab, second, window, cx);
    });
    h.cross_begin(source, tab);
    h.runtime
        .lock()
        .close_pane(h.workspace, tab, second)
        .unwrap();
    h.wait_for("another client closes the focused source pane", |cx| {
        pane_view(cx, source, tab, second).is_none()
    });
    assert!(h.update_view(source, |view, _, _| view.reorder.is_some()));
    h.cross_release(source);
    h.cross_idle(destination, first);
    assert!(h.update_view(destination, |view, _, _| {
        view.tabs
            .iter()
            .find(|view| view.id == tab)
            .unwrap()
            .panes
            .len()
            == 1
    }));
}

#[gpui::test]
fn cross_window_tear_off_pill_uses_free_space_after_tabs_and_add_button(
    cx: &mut TestAppContext,
) {
    for position in [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
        TabPosition::Titlebar,
    ] {
        let mut h = DesktopHarness::with_config(cx, |config| {
            config.tabs.always_show = true;
            config.tabs.position = position;
            config.tabs.width = TabWidth::Fit;
        });
        let tab = h.open_tab(ECHO);
        let _ = h.open_tab(ECHO);
        let source = h.attach_window();
        h.activate(source, tab);
        let _locator = h.cross_locator();
        h.cross_begin(source, tab);
        let plus = VisualTestContext::from_window(source, &h.cx)
            .debug_bounds("new-tab")
            .expect("new-tab was drawn");
        h.update_view(source, |view, window, _| {
            let pill = view
                .drawn_tear_off
                .borrow()
                .expect("tear-off pill was drawn");
            let strip = view.tab_strip(window);
            for index in 0..view.tabs.len() {
                assert!(
                    !pill.intersects(&strip.label(index)),
                    "{position:?}: pill {pill:?} overlaps tab {index}"
                );
            }
            assert!(
                !pill.intersects(&plus),
                "{position:?}: pill {pill:?} overlaps + {plus:?}"
            );
        });
    }
}

#[gpui::test]
fn a_narrow_titlebar_tear_off_pill_turns_compact_instead_of_covering_tabs(
    cx: &mut TestAppContext,
) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.tabs.position = TabPosition::Titlebar;
    });
    let tab = h.open_tab(ECHO);
    let _ = h.open_tab(ECHO);
    let source = h.attach_window();
    h.cx.simulate_window_resize(source, size(px(456.0), px(353.0)));
    h.activate(source, tab);
    let _locator = h.cross_locator();
    h.cross_begin(source, tab);
    let plus = VisualTestContext::from_window(source, &h.cx)
        .debug_bounds("new-tab")
        .expect("new-tab was drawn");
    h.update_view(source, |view, window, _| {
        let pill = view
            .drawn_tear_off
            .borrow()
            .expect("tear-off pill was drawn");
        let strip = view.tab_strip(window);
        assert_eq!(
            strip.max_offset(),
            px(0.0),
            "fixture strip must not overflow"
        );
        for index in 0..view.tabs.len() {
            assert!(
                !pill.intersects(&strip.label(index)),
                "pill {pill:?} overlaps tab {index}"
            );
        }
        assert!(!pill.intersects(&plus), "pill {pill:?} overlaps + {plus:?}");
        assert_eq!(super::super::cross_window::tear_off_label(pill), "↗");
    });
}

#[gpui::test]
fn cross_window_lone_tab_command_is_unavailable_in_palette_and_invocation(
    cx: &mut TestAppContext,
) {
    let mut h = DesktopHarness::new(cx);
    let tab = h.open_tab(ECHO);
    let source = h.attach_window();
    h.activate(source, tab);
    let unavailable = |result: Result<(), CommandError>| {
        assert!(
            matches!(result, Err(CommandError::Unavailable(reason)) if reason == "The tab is already alone in its window.")
        );
    };
    let availability = |h: &mut DesktopHarness| {
        h.update_view(source, |view, window, cx| {
            let target = PaletteTarget {
                session: None,
                workspace: view.workspace_id(cx),
                tab: view.active_tab(cx),
                terminal: None,
                terminal_view: None,
                contexts: window.context_stack(),
            };
            view.command_availability(ids::MOVE_TAB_TO_NEW_WINDOW, &target, cx)
        })
    };
    unavailable(availability(&mut h));
    unavailable(h.invoke(source, ids::MOVE_TAB_TO_NEW_WINDOW).map(|_| ()));
    h.update_view(source, |view, window, cx| {
        unavailable(
            view.move_tab_to_new_window(
                &CommandInvocation::new(ids::MOVE_TAB_TO_NEW_WINDOW, vec![]),
                window,
                cx,
            )
            .map(|_| ()),
        );
    });
    assert_eq!(h.cx.update(|cx| cx.windows().len()), 1);
    let _ = h.open_tab(ECHO);
    h.wait_for("command becomes available after adding a tab", |cx| {
        source
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .unwrap()
                    .read(cx)
                    .tabs
                    .len()
                    == 2
            })
            .unwrap()
    });
    assert!(availability(&mut h).is_ok());
}

#[gpui::test]
fn cross_window_incoming_title_chip_stays_inside_a_bottom_bar_window(
    cx: &mut TestAppContext,
) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.tabs.position = TabPosition::Bottom;
    });
    let tab = h.open_tab(ECHO);
    let source = h.attach_window();
    let (destination, _, _) = h.private_window();
    let locator = h.cross_locator();
    h.activate(source, tab);
    let pointer = h.update_view(destination, |_, window, _| {
        point(
            window.viewport_size().width - px(1.0),
            window.viewport_size().height - px(1.0),
        )
    });
    locator
        .0
        .set(Located::Window(destination.window_id(), pointer));
    h.cross_begin(source, tab);
    let chip = VisualTestContext::from_window(destination, &h.cx)
        .debug_bounds("drag-title-chip")
        .expect("incoming chip was drawn");
    h.update_view(destination, |_, window, _| {
        assert!(chip.left() >= px(0.0) && chip.top() >= px(0.0));
        assert!(
            chip.right() <= window.viewport_size().width
                && chip.bottom() <= window.viewport_size().height,
            "chip {chip:?} clips at the window edge"
        );
        assert_eq!(chip.right(), pointer.x - px(12.0));
        assert_eq!(chip.bottom(), pointer.y - px(12.0));
    });
}

#[gpui::test]
fn cross_window_overflowed_tear_off_pill_ends_before_the_add_control(
    cx: &mut TestAppContext,
) {
    for position in [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
        TabPosition::Titlebar,
    ] {
        let mut h = DesktopHarness::with_config(cx, |config| {
            config.tabs.always_show = true;
            config.tabs.position = position;
            config.tabs.width = TabWidth::Fill;
        });
        let tab = h.open_tab(ECHO);
        for _ in 0..5 {
            let _ = h.open_tab(ECHO);
        }
        let source = h.attach_window();
        h.cx.simulate_window_resize(source, size(px(400.0), px(170.0)));
        h.activate(source, tab);
        let _locator = h.cross_locator();
        h.cross_begin(source, tab);
        let plus = VisualTestContext::from_window(source, &h.cx)
            .debug_bounds("new-tab")
            .expect("add control was drawn");
        h.update_view(source, |view, window, _| {
            let strip = view.tab_strip(window);
            assert!(
                strip.max_offset() > px(0.0),
                "fixture strip must overflow"
            );
            let pill = view
                .drawn_tear_off
                .borrow()
                .expect("tear-off pill was drawn");
            assert!(
                !pill.intersects(&plus),
                "{position:?}: pill overlaps add control"
            );
            if strip.vertical {
                assert_eq!(pill.bottom(), strip.bounds.bottom());
            } else {
                assert_eq!(pill.right(), strip.bounds.right());
            }
        });
    }
}

#[gpui::test]
fn cross_window_navigation_interrupt_ends_the_pane_gesture(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, workspace, origin, locator) =
        DesktopHarness::cross_fixture(cx);
    let sprung = h
        .runtime
        .lock()
        .open_tab(workspace, &fixture(ECHO))
        .unwrap()
        .tab
        .id;
    h.wait_for("second destination tab", |cx| {
        terminal(cx, destination, sprung).is_some()
    });
    let label = h.update_view(destination, |view, window, _| {
        view.tab_strip(window).label(1).center()
    });
    locator
        .0
        .set(Located::Window(destination.window_id(), label));
    let pane = h.cross_pane(tab);
    h.cross_begin_pane(source, tab, pane);
    h.update_view(destination, |view, window, cx| {
        view.advance_incoming_spring(
            Instant::now() + SPRING_LOAD_DELAY,
            window,
            cx,
        );
    });
    assert_eq!(
        h.update_view(destination, |view, _, cx| view.active_tab(cx)),
        Some(sprung)
    );
    h.invoke(source, ids::NEXT_TAB).unwrap();
    h.cx.run_until_parked();
    assert!(h.update_view(source, |view, _, _| view.pane_drag.is_none()));
    assert!(
        h.cx.update(|cx| cx.global::<Desktop>().cross.drag.is_none()),
        "navigation must end the shared gesture"
    );
    assert!(h.update_view(destination, |view, _, _| view.incoming.is_none()));
    assert_eq!(
        h.update_view(destination, |view, _, cx| view.active_tab(cx)),
        Some(origin)
    );
    h.update_view(source, |view, window, cx| {
        view.select_tab(tab, true, window, cx);
    });
    h.update_view(destination, |view, window, cx| {
        view.select_tab(sprung, false, window, cx);
    });
    let label = h.update_view(destination, |view, window, _| {
        view.tab_strip(window).label(0).center()
    });
    locator
        .0
        .set(Located::Window(destination.window_id(), label));
    h.cross_begin_pane(source, tab, pane);
    h.update_view(destination, |view, window, cx| {
        view.advance_incoming_spring(
            Instant::now() + SPRING_LOAD_DELAY,
            window,
            cx,
        );
    });
    assert_eq!(
        h.update_view(destination, |view, _, cx| view.active_tab(cx)),
        Some(origin)
    );
    h.update_view(source, super::super::WorkspaceView::cancel_pane_drag);
    h.cx.run_until_parked();
    assert_eq!(
        h.update_view(destination, |view, _, cx| view.active_tab(cx)),
        Some(sprung),
        "the next gesture must spring back to its own origin"
    );
}

impl DesktopHarness {
    fn finish_queued_cross_closes(
        &mut self,
        source: AnyWindowHandle,
        destination: AnyWindowHandle,
    ) {
        self.runtime.worker_hooks.hold(Stage::Subscribe);
        self.runtime.worker_hooks.release(Stage::Transfer);
        let runtime = self.runtime.clone();
        self.wait_for(
            "source close runs while destination waits for install",
            |cx| {
                !cx.windows().contains(&source)
                    && runtime.worker_hooks.held(Stage::Subscribe) == 1
            },
        );
        assert_eq!(
            runtime.worker_hooks.entered(Stage::Assess),
            1,
            "destination close must wait for its viewer install"
        );
        assert!(self.update_view(destination, |view, _, _| view.busy
            && view.transfer.is_some()));
        runtime.worker_hooks.release(Stage::Subscribe);
        self.wait_closed(destination);
        assert_eq!(runtime.worker_hooks.entered(Stage::Assess), 2);
        assert_eq!(self.close_failures(), 0);
    }

    fn retry_moved_install(&mut self, destination: AnyWindowHandle) {
        self.update_view(destination, |view, window, cx| {
            let id = view.notices.newest().unwrap();
            let notice = &view.notices.get(id).unwrap().content;
            assert_eq!(notice.title, "Cannot show moved tab");
            assert_eq!(notice.actions[0].label, "Try Again");
            view.run_notice_action(id, None, window, cx).unwrap();
        });
    }
}

fn failed_created_install(cx: &mut TestAppContext, command: bool) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
    });
    let tab = h.open_tab(ECHO);
    let _ = h.open_tab(ECHO);
    let source = h.attach_window();
    h.activate(source, tab);
    let pane = h.cross_pane(tab);
    let _locator = h.cross_locator();
    h.runtime.worker_hooks.fail(Stage::Subscribe);
    if command {
        h.invoke(source, ids::MOVE_TAB_TO_NEW_WINDOW).unwrap();
    } else {
        h.cross_begin(source, tab);
        h.cross_release(source);
    }
    let mut destination = None;
    h.wait_for("created destination reports failed install", |cx| {
        destination = cx.windows().into_iter().find(|handle| *handle != source);
        destination.is_some_and(|handle| {
            notices(cx, handle)
                .iter()
                .any(|notice| notice.contains("Cannot show moved tab"))
        })
    });
    let destination = destination.unwrap();
    h.wait_for("failed destination visible and active", |cx| {
        destination
            .update(cx, |_, window, _| {
                window.is_window_active() && window.is_visible()
            })
            .unwrap()
    });
    assert!(h.update_view(destination, |view, _, cx| !view.busy
        && view.transfer.is_none()
        && view.tabs.is_empty()
        && !view.has_nothing_to_install(cx)));
    let workspace = h.cx.update(|cx| {
        cx.global::<Desktop>()
            .windows
            .record(destination.window_id())
            .unwrap()
            .workspace
            .unwrap()
    });
    assert!(contains(&h.runtime.lock(), workspace, tab));
    // Complete the automatic emptied-close pipeline if it was owed. It must
    // refuse against projected/runtime membership and leave the host open.
    h.cx.run_until_parked();
    assert!(h.cx.update(|cx| cx.windows().contains(&destination)));
    h.retry_moved_install(destination);
    h.cross_idle(destination, pane);
    h.type_line(destination, "created-retry-alive");
    h.wait_text(destination, tab, "got:created-retry-alive");
}

#[gpui::test]
fn cross_window_failed_tear_off_install_keeps_the_host_and_terminal(
    cx: &mut TestAppContext,
) {
    failed_created_install(cx, false);
}

#[gpui::test]
fn cross_window_failed_command_install_keeps_the_host_and_terminal(
    cx: &mut TestAppContext,
) {
    failed_created_install(cx, true);
}

#[gpui::test]
fn cross_window_failed_install_resumes_a_queued_destination_close(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, _) =
        DesktopHarness::cross_fixture(cx);
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.hold(Stage::Subscribe);
    h.runtime.worker_hooks.fail(Stage::Subscribe);
    h.cross_release(source);
    let runtime = h.runtime.clone();
    h.wait_for("destination subscription starts", |_| {
        runtime.worker_hooks.held(Stage::Subscribe) == 1
    });
    h.update_view(destination, |view, window, cx| {
        view.request_close(CloseTarget::Window, window, cx);
    });
    assert_eq!(runtime.worker_hooks.entered(Stage::Assess), 0);
    runtime.worker_hooks.hold(Stage::Assess);
    h.runtime.worker_hooks.release(Stage::Subscribe);
    h.wait_for("failed install resumes queued close", |_| {
        runtime.worker_hooks.held(Stage::Assess) == 1
    });
    assert!(h.update_view(destination, |view, _, _| view.transfer.is_none()));
    assert!(h.cx.update(|cx| {
        notices(cx, destination)
            .iter()
            .any(|notice| notice.contains("Cannot show moved tab"))
    }));
    runtime.worker_hooks.release(Stage::Assess);
    h.wait_closed(destination);
    assert_eq!(runtime.worker_hooks.entered(Stage::Assess), 1);
    assert_eq!(h.close_failures(), 0);
}

fn pending_pane_leaves_workspace(cx: &mut TestAppContext, close: bool) {
    let (mut h, source, tab, destination, workspace, _, _) =
        DesktopHarness::cross_fixture(cx);
    let pane = h.cross_pane(tab);
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.hold(Stage::Subscribe);
    h.cross_release(source);
    let runtime = h.runtime.clone();
    h.wait_for("destination subscription starts", |_| {
        runtime.worker_hooks.held(Stage::Subscribe) == 1
    });
    if close {
        h.runtime.lock().close_pane(workspace, tab, pane).unwrap();
    } else {
        h.runtime
            .lock()
            .move_tab(workspace, tab, h.workspace, None)
            .unwrap();
    }
    h.wait_for("removed moved pane releases reservation", |cx| {
        destination
            .update(cx, |root, _, cx| {
                let view = root.downcast::<WorkspaceView>().unwrap();
                !view.read(cx).busy && view.read(cx).transfer.is_none()
            })
            .unwrap()
    });
    assert!(runtime.worker_hooks.held(Stage::Subscribe) >= 1);
    runtime.worker_hooks.release(Stage::Subscribe);
    h.wait_for("stale subscription finishes", |_| {
        runtime.worker_hooks.held(Stage::Subscribe) == 0
    });
    h.cx.run_until_parked();
    assert!(h.cx.update(|cx| notices(cx, destination).is_empty()));
    if !close {
        h.wait_for("pane moved back installs in source", |cx| {
            pane_view(cx, source, tab, pane).is_some()
        });
        h.update_view(source, |view, window, cx| {
            view.select_tab(tab, true, window, cx);
        });
        h.activate(source, tab);
        h.type_line(source, "moved-back-alive");
        h.wait_text(source, tab, "got:moved-back-alive");
    }
}

#[gpui::test]
fn cross_window_removed_pending_pane_settles_before_subscription_returns(
    cx: &mut TestAppContext,
) {
    pending_pane_leaves_workspace(cx, true);
}

#[gpui::test]
fn cross_window_relocated_pending_pane_settles_before_subscription_returns(
    cx: &mut TestAppContext,
) {
    pending_pane_leaves_workspace(cx, false);
}

#[gpui::test]
fn cross_window_successful_pane_edge_drop_clears_zoom_without_history(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, workspace, anchor, locator) =
        DesktopHarness::cross_fixture(cx);
    let pane = h.cross_pane(tab);
    let target = h.cross_pane(anchor);
    let second = h
        .runtime
        .lock()
        .split_pane(
            workspace,
            anchor,
            target,
            SplitDirection::Right,
            &fixture(ECHO),
        )
        .unwrap()
        .pane
        .id;
    h.wait_for("destination split installs", |cx| {
        pane_view(cx, destination, anchor, second).is_some()
    });
    h.update_view(destination, |view, _, cx| {
        view.tabs
            .iter_mut()
            .find(|view| view.id == anchor)
            .unwrap()
            .zoomed = Some(target);
        cx.notify();
    });
    h.cx.run_until_parked();
    let history = h.cx.update(|cx| {
        cx.global::<Desktop>()
            .windows
            .record(destination.window_id())
            .unwrap()
            .layout_history
            .clone()
    });
    let frame = h.update_view(destination, |view, _, _| {
        view.drawn_frame(target).unwrap()
    });
    locator.0.set(Located::Window(
        destination.window_id(),
        point(frame.right() - px(3.0), frame.center().y),
    ));
    h.cross_begin_pane(source, tab, pane);
    h.cross_release_pane(source);
    h.cross_idle(destination, pane);
    assert_eq!(
        h.update_view(destination, |view, _, _| view
            .tabs
            .iter()
            .find(|view| view.id == anchor)
            .unwrap()
            .zoomed),
        None
    );
    assert_eq!(
        h.cx.update(|cx| cx
            .global::<Desktop>()
            .windows
            .record(destination.window_id())
            .unwrap()
            .layout_history
            .clone()),
        history
    );
}

#[gpui::test]
fn cross_window_new_gesture_from_same_source_rejects_an_old_generation(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, locator) =
        DesktopHarness::cross_fixture(cx);
    h.cross_begin(source, tab);
    let late =
        h.cx.update(|cx| cx.global::<Desktop>().cross.drag.clone().unwrap());
    h.update_view(source, super::super::WorkspaceView::cancel_reorder);
    h.cx.run_until_parked();
    locator.0.set(Located::Blocked);
    h.cross_begin(source, tab);
    let pointer = h.cross_slot(destination);
    h.update_view(destination, |view, window, cx| {
        view.receive_incoming(late, pointer, window, cx);
    });
    assert!(
        h.update_view(destination, |view, _, _| view.incoming.is_none()),
        "an old gesture from the same source must not restore its preview"
    );
    h.update_view(source, super::super::WorkspaceView::cancel_reorder);
}

#[gpui::test]
fn cross_window_failed_install_resumes_the_sources_queued_pane_close(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, _) =
        DesktopHarness::cross_fixture(cx);
    let pane = h.cross_pane(tab);
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.hold(Stage::Transfer);
    h.runtime.worker_hooks.fail(Stage::Subscribe);
    h.cross_release(source);
    let runtime = h.runtime.clone();
    h.wait_for("transfer starts before pane close", |_| {
        runtime.worker_hooks.held(Stage::Transfer) == 1
    });
    h.update_view(source, |view, window, cx| {
        view.request_close(CloseTarget::Pane { tab, pane }, window, cx);
    });
    runtime.worker_hooks.release(Stage::Transfer);
    h.wait_for("pane close follows failed install", |cx| {
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .pane_tab(pane)
            .is_none()
    });
    assert!(h.update_view(destination, |view, _, _| view.transfer.is_none()));
    assert_eq!(runtime.worker_hooks.entered(Stage::Assess), 1);
    assert_eq!(h.close_failures(), 0);
}

#[gpui::test]
fn cross_window_stale_install_notices_exclude_unrelated_panes_and_termination(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, anchor, _) =
        DesktopHarness::cross_fixture(cx);
    h.cross_begin(source, tab);
    h.runtime.worker_hooks.hold(Stage::Subscribe);
    h.cross_release(source);
    let runtime = h.runtime.clone();
    h.wait_for("moved pane installation starts", |_| {
        runtime.worker_hooks.held(Stage::Subscribe) == 1
    });
    h.revoke(destination);
    h.wait_for("unrelated revoked tab replacement starts", |_| {
        runtime.worker_hooks.held(Stage::Subscribe) == 2
    });
    h.update_view(destination, |view, window, cx| {
        assert!(view.transfer.is_some());
        let unrelated = view.pane_targets(anchor, cx);
        assert_eq!(unrelated.len(), 1);
        let seq = cx.global::<Desktop>().hierarchy.state().seq();
        view.apply_tab_viewers(
            anchor,
            unrelated
                .iter()
                .map(|target| (target, Err(RuntimeError::Stopped.into())))
                .collect(),
            true,
            seq,
            window,
            cx,
        );
        assert!(
            view.notices.is_empty(),
            "a pending transfer must not make an unrelated stale install noisy"
        );
        let moved = view.pane_targets(tab, cx);
        assert_eq!(moved.len(), 1);
        // Exercise the reporting boundary even with an applying result. The
        // normal subscription completion also rejects all results in teardown.
        runtime.terminating.store(true, Ordering::Release);
        view.apply_tab_viewers(
            tab,
            moved
                .iter()
                .map(|target| (target, Err(RuntimeError::Stopped.into())))
                .collect(),
            true,
            seq,
            window,
            cx,
        );
        runtime.terminating.store(false, Ordering::Release);
        assert!(
            view.notices.is_empty(),
            "teardown must not post a moved-pane install notice"
        );
    });
    runtime.worker_hooks.release(Stage::Subscribe);
}
