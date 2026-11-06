//! The floating preview with the platform stood in by test hooks: window
//! tests have no `AppKit` window, so a hook decides whether a show
//! succeeds.
use super::super::super::drag_preview::PreviewMode;
use super::*;

impl DesktopHarness {
    fn preview_hooks(&mut self, visible: bool, fail_open: bool) {
        self.cx.update(|cx| {
            let hooks = &mut cx.global_mut::<Desktop>().cross.preview.hooks;
            hooks.supported = true;
            hooks.visible = visible;
            hooks.fail_open = fail_open;
        });
    }
    fn preview(&mut self) -> (Option<gpui::WindowId>, PreviewMode, bool) {
        self.cx.run_until_parked();
        self.cx.update(|cx| {
            let preview = &cx.global::<Desktop>().cross.preview;
            let id = preview.window_id();
            assert_eq!(
                id.is_some(),
                cx.windows()
                    .iter()
                    .any(|handle| Some(handle.window_id()) == id),
                "the record and the open windows agree"
            );
            (id, preview.mode(), preview.visible())
        })
    }
    fn preview_closed(&mut self, what: &str) {
        let (id, mode, visible) = self.preview();
        assert_eq!(
            (id, mode, visible),
            (None, PreviewMode::None, false),
            "{what}"
        );
        assert!(
            self.cx
                .update(|cx| cx.windows().iter().all(|handle| handle
                    .downcast::<WorkspaceView>()
                    .is_some())),
            "{what}: no preview window is left"
        );
    }
    fn chip_drawn(&mut self, window: AnyWindowHandle) -> bool {
        VisualTestContext::from_window(window, &self.cx)
            .debug_bounds("drag-title-chip")
            .is_some()
    }
}

#[gpui::test]
fn drag_preview_modes_follow_the_pointer_and_close_at_release(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, locator) =
        DesktopHarness::cross_fixture(cx);
    h.preview_hooks(true, false);
    let inside = h.cross_slot(source);
    locator.0.set(Located::Window(source.window_id(), inside));
    h.cross_begin(source, tab);
    assert_eq!(h.preview().1, PreviewMode::None, "inside the source");
    let slot = h.cross_slot(destination);
    locator
        .0
        .set(Located::Window(destination.window_id(), slot));
    h.update_view(source, |view, window, cx| {
        view.update_reorder(point(px(-10.0), px(-10.0)), window, cx);
    });
    let (id, mode, visible) = h.preview();
    assert!(id.is_some());
    assert_eq!((mode, visible), (PreviewMode::Chip, true));
    assert!(
        h.update_view(destination, |view, _, _| view
            .incoming
            .as_ref()
            .is_some_and(
                |incoming| incoming.target == Some(DropTarget::Slot(0))
            )),
        "the destination keeps its target"
    );
    assert!(!h.chip_drawn(destination), "the preview draws the chip");
    locator.0.set(Located::Outside);
    h.update_view(source, |view, window, cx| {
        view.update_reorder(point(px(-20.0), px(-20.0)), window, cx);
    });
    let (same, mode, _) = h.preview();
    assert_eq!(same, id, "one preview window per drag");
    assert_eq!(mode, PreviewMode::Window);
    h.update_view(source, |view, _, cx| {
        assert!(view.tear_off_cue(cx), "a release still tears off");
        assert!(
            view.drawn_tear_off.borrow().is_none(),
            "no pill under the preview"
        );
        assert_eq!(view.cross_drop_label(cx).as_deref(), Some("tear_off"));
    });
    locator.0.set(Located::Blocked);
    h.update_view(source, |view, window, cx| {
        view.update_reorder(point(px(-30.0), px(-30.0)), window, cx);
    });
    assert_eq!(h.preview().1, PreviewMode::Dimmed);
    locator.0.set(Located::Window(source.window_id(), inside));
    h.update_view(source, |view, window, cx| {
        view.update_reorder(inside, window, cx);
    });
    let (kept, mode, visible) = h.preview();
    assert_eq!(
        (kept, mode, visible),
        (id, PreviewMode::None, false),
        "hidden inside the source"
    );
    locator
        .0
        .set(Located::Window(destination.window_id(), slot));
    h.cross_release(source);
    h.preview_closed("release");
    let pane = h.cross_pane(tab);
    h.cross_idle(destination, pane);
}

#[gpui::test]
fn drag_preview_closes_on_escape_and_when_either_window_closes(
    cx: &mut TestAppContext,
) {
    let (mut h, source, tab, destination, _, _, locator) =
        DesktopHarness::cross_fixture(cx);
    h.preview_hooks(true, false);
    h.cross_begin(source, tab);
    assert_eq!(h.preview().1, PreviewMode::Chip);
    h.update_view(source, WorkspaceView::cancel_reorder);
    h.preview_closed("Escape");
    h.cross_begin(source, tab);
    assert_eq!(h.preview().1, PreviewMode::Chip);
    h.remove_window(destination);
    h.preview_closed("destination closed");
    locator.0.set(Located::Outside);
    h.cross_begin(source, tab);
    assert_eq!(h.preview().1, PreviewMode::Window);
    h.remove_window(source);
    h.preview_closed("source closed");
}

#[gpui::test]
fn drag_preview_lone_tab_shows_the_dimmed_chip_and_terminal_windows_only(
    cx: &mut TestAppContext,
) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
    });
    let tab = h.open_tab(ECHO);
    let source = h.attach_window();
    let (destination, _, _) = h.private_window();
    h.activate(source, tab);
    let _locator = h.cross_locator();
    h.preview_hooks(true, false);
    h.cross_begin(source, tab);
    let (id, mode, _) = h.preview();
    let id = id.expect("a preview window is open");
    assert_eq!(mode, PreviewMode::Dimmed);
    assert_eq!(h.cx.update(|cx| cx.windows().len()), 3);
    let state = h.cx.update(super::super::super::cross_window::smoke_state);
    assert!(state.starts_with("windows=2\npreview=dimmed"), "{state}");
    for index in [0, 1] {
        assert!(state.contains(&format!("w{index}.native=")), "{state}");
    }
    assert!(!state.contains("w2."), "the preview has no index: {state}");
    h.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        assert!(desktop.windows.record(id).is_none(), "no window record");
        assert!(
            desktop
                .cross
                .windows
                .iter()
                .all(|(window, _)| *window != id),
            "not a cross-window target"
        );
        assert_eq!(desktop.windows.records().count(), 2);
    });
    let _ = destination;
    h.cross_release(source);
    h.preview_closed("release of a lone tab");
    assert_eq!(h.cx.update(|cx| cx.windows().len()), 2);
}

#[gpui::test]
fn drag_preview_a_failed_or_unshown_preview_keeps_the_in_window_cues(
    cx: &mut TestAppContext,
) {
    for (visible, fail_open) in [(false, false), (true, true)] {
        let (mut h, source, tab, destination, _, _, locator) =
            DesktopHarness::cross_fixture(cx);
        h.preview_hooks(visible, fail_open);
        h.cross_begin(source, tab);
        let (id, _, shown) = h.preview();
        assert_eq!(id.is_some(), !fail_open);
        assert!(!shown);
        assert!(
            h.chip_drawn(destination),
            "visible {visible}, failed {fail_open}: the destination draws the chip"
        );
        locator.0.set(Located::Outside);
        h.update_view(source, |view, window, cx| {
            view.update_reorder(point(px(-10.0), px(-10.0)), window, cx);
        });
        h.cx.run_until_parked();
        assert!(
            h.update_view(source, |view, _, _| view
                .drawn_tear_off
                .borrow()
                .is_some()),
            "visible {visible}, failed {fail_open}: the pill draws"
        );
        h.update_view(source, WorkspaceView::cancel_reorder);
        h.preview_closed("cancel");
    }
}

#[gpui::test]
fn drag_preview_hides_s1_and_turning_it_off_mid_drag_brings_the_cues_back(
    cx: &mut TestAppContext,
) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
    });
    let (source, tab, _, second) = h.split_source();
    let _locator = h.cross_locator();
    h.preview_hooks(true, false);
    h.cross_begin_pane(source, tab, second);
    assert_eq!(h.preview().1, PreviewMode::Window);
    assert!(h.update_view(source, |view, _, cx| view.tear_off_cue(cx)
        && view.drawn_pane_tear_off.borrow().is_none()));
    h.cx.update(|cx| {
        let mut changed = cx.global::<Desktop>().config.clone();
        changed.window.macos_drag_preview = false;
        finish_reload(
            cx,
            ReloadKind::Manual,
            Path::new("huterm-test.toml"),
            config::Reloaded {
                config: Ok(changed),
                inputs: config::Inputs::default(),
                missing: false,
            },
            0,
        );
    });
    let (_, mode, visible) = h.preview();
    assert_eq!((mode, visible), (PreviewMode::None, false));
    assert!(
        h.update_view(source, |view, _, _| view.pane_drag.is_some()),
        "the reload keeps the drag"
    );
    assert!(
        h.update_view(source, |view, _, _| view
            .drawn_pane_tear_off
            .borrow()
            .is_some()),
        "S1 returns with the setting off"
    );
    h.update_view(source, WorkspaceView::cancel_pane_drag);
    h.preview_closed("cancel");
}
