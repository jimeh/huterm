//! Native menus through a stand-in presenter: which menus open natively,
//! what their picks run, and when they close. `AppKit` itself is covered by
//! `smoke:macos-menus` and manual checks.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{Modifiers, MouseDownEvent};

use super::super::menu_spec::{NativeItem, NativeMenu};
use super::super::native_menus::{MenuAnchor, MenuPresenter, PresentedMenu};
use super::*;

/// One menu the presenter showed, with the sender that answers it.
struct Shown {
    menu: NativeMenu,
    anchor: MenuAnchor,
    pick: async_channel::Sender<Option<usize>>,
    closed: Rc<Cell<bool>>,
}

impl Shown {
    fn ids(&self) -> Vec<String> {
        self.menu
            .items
            .iter()
            .map(|item| match item {
                NativeItem::Item { id, enabled, .. } => {
                    format!("{id}{}", if *enabled { "" } else { "!" })
                }
                NativeItem::Separator => "-".to_owned(),
            })
            .collect()
    }

    /// Picks the item `id`, as a click on it would.
    fn pick(&self, id: &str) {
        let index = self
            .menu
            .items
            .iter()
            .position(|item| {
                matches!(item, NativeItem::Item { id: item, .. } if item == id)
            })
            .unwrap_or_else(|| panic!("no item `{id}`"));
        self.pick.try_send(Some(index)).unwrap();
    }

    fn dismiss(&self) {
        self.pick.try_send(None).unwrap();
    }
}

struct Closed(Rc<Cell<bool>>);

impl Drop for Closed {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

#[derive(Default)]
struct FakePresenter {
    shown: RefCell<Vec<Shown>>,
}

impl MenuPresenter for FakePresenter {
    fn present(
        &self,
        menu: &NativeMenu,
        anchor: MenuAnchor,
        _: &Window,
    ) -> anyhow::Result<PresentedMenu> {
        let (pick, picked) = async_channel::bounded(1);
        let closed = Rc::new(Cell::new(false));
        self.shown.borrow_mut().push(Shown {
            menu: menu.clone(),
            anchor,
            pick,
            closed: Rc::clone(&closed),
        });
        Ok(PresentedMenu {
            picked,
            _close: Box::new(Closed(closed)),
        })
    }
}

fn native_kind(cx: &mut App, window: AnyWindowHandle) -> Option<MenuKind> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx).native_menu_kind().cloned()
        })
        .ok()
        .flatten()
}

fn huterm_menu_open(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .is_ok_and(|view| view.read(cx).menu.is_some())
        })
        .unwrap_or(false)
}

impl DesktopHarness {
    /// A focused window on an [`ECHO`] tab whose menus open through the
    /// returned presenter.
    fn native_window(&mut self) -> (AnyWindowHandle, TabId, Rc<FakePresenter>) {
        let presenter = Rc::new(FakePresenter::default());
        let shared: Rc<dyn MenuPresenter> = presenter.clone();
        self.cx.update(|cx| {
            cx.global_mut::<Desktop>().menu_presenter = Some(shared);
        });
        let tab = self.open_tab(ECHO);
        let window = self.attach_window();
        self.activate(window, tab);
        self.wait_text(window, tab, "READY");
        (window, tab, presenter)
    }

    /// A right press on the terminal, with no release: the native menu
    /// takes the release.
    fn right_press(
        &mut self,
        window: AnyWindowHandle,
        at: gpui::Point<Pixels>,
    ) {
        VisualTestContext::from_window(window, &self.cx).simulate_event(
            MouseDownEvent {
                button: MouseButton::Right,
                position: at,
                modifiers: Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            },
        );
    }

    /// Waits for the presenter to have shown `count` menus.
    fn wait_shown(&mut self, presenter: &FakePresenter, count: usize) {
        // A right-click's menu opens once its link lookup answers.
        self.wait_for("the native menu to open", |_| {
            presenter.shown.borrow().len() >= count
        });
        assert_eq!(presenter.shown.borrow().len(), count);
    }
}

#[gpui::test]
fn a_right_click_opens_the_native_terminal_menu_and_runs_its_pick(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, presenter) = harness.native_window();
    let centre = harness.terminal_centre(window);
    harness.right_press(window, centre);
    harness.wait_shown(&presenter, 1);
    harness.cx.update(|cx| {
        assert!(!huterm_menu_open(cx, window), "no Huterm menu draws");
        assert!(matches!(
            native_kind(cx, window),
            Some(MenuKind::Terminal { .. })
        ));
    });
    {
        let shown = presenter.shown.borrow();
        assert_eq!(shown[0].anchor, MenuAnchor::Pointer(centre));
        let ids = shown[0].ids();
        assert_eq!(ids[..3], ["copy!", "paste", "select_all"]);
        assert!(
            shown[0].menu.dark,
            "the default dark theme gets a dark menu"
        );
        shown[0].pick("select_all");
    }
    harness.wait_for("the pick to run", |cx| {
        native_kind(cx, window).is_none()
            && terminal(cx, window, tab)
                .is_some_and(|terminal| terminal.read(cx).selection.is_some())
    });
}

#[gpui::test]
fn a_right_click_after_a_native_menu_closes_opens_another(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, presenter) = harness.native_window();
    let centre = harness.terminal_centre(window);
    harness.right_press(window, centre);
    harness.wait_shown(&presenter, 1);
    presenter.shown.borrow()[0].dismiss();
    harness
        .wait_for("the menu to close", |cx| native_kind(cx, window).is_none());
    harness.cx.update(|cx| {
        let terminal = terminal(cx, window, tab).unwrap();
        assert!(
            !terminal.read(cx).owns_pointer_gesture(),
            "the press the menu took is released"
        );
    });
    harness.right_press(window, centre);
    harness.wait_shown(&presenter, 2);
}

#[gpui::test]
fn a_native_tab_menu_acts_on_its_tab_and_closes_with_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, first, presenter) = harness.native_window();
    let second = harness.open_tab(ECHO);
    let third = harness.open_tab(ECHO);
    harness.wait_for("the window to show three tabs", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .is_ok_and(|view| view.read(cx).tabs.len() == 3)
            })
            .unwrap_or(false)
    });
    let open = |harness: &mut DesktopHarness, tab: TabId| {
        harness.update_view(window, |view, window, cx| {
            view.open_tab_menu(tab, point(px(40.0), px(10.0)), window, cx);
        });
    };
    open(&mut harness, second);
    harness.wait_shown(&presenter, 1);
    presenter.shown.borrow()[0].pick("close_tab");
    harness.wait_for("the targeted tab to close", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    let view = view.read(cx);
                    view.tabs.iter().all(|record| record.id != second)
                        && view.active_tab(cx) == Some(first)
                })
            })
            .unwrap_or(false)
    });
    // A tab that closes while its menu is open closes the menu.
    open(&mut harness, third);
    harness.wait_shown(&presenter, 2);
    harness.close_tab(third);
    harness.wait_for("the menu to close with its tab", |cx| {
        native_kind(cx, window).is_none()
    });
    assert!(presenter.shown.borrow()[1].closed.get());
}

#[gpui::test]
fn the_native_window_menu_lists_copy_and_paste_and_opens_below_its_button(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _, presenter) = harness.native_window();
    harness.invoke(window, ids::OPEN_MENU).unwrap();
    harness.wait_shown(&presenter, 1);
    let shown = presenter.shown.borrow();
    assert!(matches!(shown[0].anchor, MenuAnchor::Below(_)));
    let ids = shown[0].ids();
    let redo = ids.iter().position(|id| id == "redo!").unwrap();
    assert_eq!(
        ids[redo + 1..redo + 4],
        ["copy!", "paste", "toggle_fullscreen"]
    );
    drop(shown);
    // A second open waits for the first menu to close.
    assert!(harness.invoke(window, ids::OPEN_MENU).is_err());
    assert_eq!(presenter.shown.borrow().len(), 1);
}

#[gpui::test]
fn turning_native_menus_off_draws_huterms_own_menus(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.macos_native_menus = false;
    });
    let (window, _, presenter) = harness.native_window();
    harness.invoke(window, ids::OPEN_MENU).unwrap();
    harness.cx.update(|cx| {
        assert!(huterm_menu_open(cx, window));
        assert_eq!(native_kind(cx, window), None);
    });
    assert!(presenter.shown.borrow().is_empty());
}

#[gpui::test]
fn a_native_pane_menu_closes_when_its_pane_loses_focus(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, presenter) = harness.native_window();
    let first = harness.first_pane(tab);
    let right = harness.split(tab, first, SplitDirection::Right, ECHO);
    harness.wait_for("the window to show both panes", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx)
                        .active_tab_view(cx)
                        .is_some_and(|view| view.pane(right).is_some())
                })
            })
            .unwrap_or(false)
    });
    // Opening the menu focuses its pane, as a press on its bar does.
    harness.update_view(window, |view, window, cx| {
        view.open_pane_menu(tab, right, point(px(400.0), px(40.0)), window, cx);
    });
    harness.wait_shown(&presenter, 1);
    assert!(
        presenter.shown.borrow()[0]
            .ids()
            .contains(&"rename_pane".to_owned())
    );
    harness.invoke(window, ids::FOCUS_PANE_LEFT).unwrap();
    harness.wait_for("the pane menu to close", |cx| {
        native_kind(cx, window).is_none()
    });
    assert!(presenter.shown.borrow()[0].closed.get());
}

#[gpui::test]
fn a_native_terminal_menu_closes_when_focus_leaves_its_pane(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, presenter) = harness.native_window();
    let first = harness.first_pane(tab);
    let second = harness.split(tab, first, SplitDirection::Right, ECHO);
    harness.wait_for("the second pane's shell", |cx| {
        pane_screen(cx, window, tab, second)
            .is_some_and(|text| text.contains("READY"))
    });
    let left = harness.terminal_centre(window);
    harness.right_press(window, left);
    harness.wait_shown(&presenter, 1);
    harness.invoke(window, ids::FOCUS_PANE_RIGHT).unwrap();
    harness.wait_for("the terminal menu to close", |cx| {
        native_kind(cx, window).is_none()
    });
    assert!(presenter.shown.borrow()[0].closed.get());
    // A pick that still arrives from the closed menu runs nothing, on
    // either pane.
    presenter.shown.borrow()[0].pick("select_all");
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        for pane in [first, second] {
            let view = pane_view(cx, window, tab, pane).unwrap();
            assert!(view.read(cx).selection.is_none(), "{pane:?} selected");
        }
    });
}

/// The open palette's field in `window`.
fn palette_field(cx: &mut App, window: AnyWindowHandle) -> Entity<Input> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap();
            let palette = view.read(cx).palette.clone().expect("open");
            palette.read(cx).field()
        })
        .unwrap()
}

/// A right-click on the palette's field opens its Edit menu natively, as
/// the terminal's menus open. The field keeps focus while the menu tracks,
/// and the pick edits the field.
#[gpui::test]
fn a_palette_fields_menu_opens_natively_and_edits_the_field(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _, presenter) = harness.native_window();
    harness.invoke(window, ids::OPEN_COMMAND_PALETTE).unwrap();
    harness.press(window, "a b c");
    let field = harness.cx.update(|cx| palette_field(cx, window));
    let (_, bounds) = harness
        .cx
        .update(|cx| field.read(cx).painted_caret())
        .expect("the field painted");
    let on_text = gpui::point(bounds.left() + px(1.0), bounds.center().y);
    harness.right_press(window, on_text);
    harness.wait_shown(&presenter, 1);
    let field_focused = |cx: &mut App| {
        window
            .update(cx, |_, window, cx| {
                field.read(cx).focus_handle(cx).is_focused(window)
            })
            .unwrap()
    };
    harness.cx.update(|cx| {
        assert!(!huterm_menu_open(cx, window), "no Huterm menu draws");
        assert!(matches!(native_kind(cx, window), Some(MenuKind::Field(_))));
        assert!(field_focused(cx), "the field keeps focus");
    });
    {
        let shown = presenter.shown.borrow();
        assert_eq!(shown[0].anchor, MenuAnchor::Pointer(on_text));
        assert_eq!(
            shown[0].ids(),
            [
                "text_cut!",
                "text_copy!",
                "text_paste",
                "-",
                "text_select_all"
            ]
        );
        shown[0].pick("text_select_all");
    }
    harness.wait_for("the pick to select the text", |cx| {
        native_kind(cx, window).is_none()
            && field.read(cx).selection() == (0..3)
    });
    harness.cx.update(|cx| {
        assert!(field_focused(cx));
        assert!(palette_state(cx, window).is_some(), "the palette stays");
    });
}

/// A Settings field's Edit menu opens natively too. Focus never leaves the
/// field, so its draft is not saved while the menu tracks, and the pick
/// edits the field.
#[gpui::test]
fn a_settings_fields_menu_opens_natively_and_keeps_the_draft(
    cx: &mut TestAppContext,
) {
    use super::settings::{ScratchConfig, settled, with_view, with_window};
    use super::settings_controls::{focus, keys, show, visual};

    let path = "terminal.scrollback_rows";
    let file = ScratchConfig::new("native-field-menu", "");
    let mut harness = DesktopHarness::new(cx);
    let presenter = Rc::new(FakePresenter::default());
    let shared: Rc<dyn MenuPresenter> = presenter.clone();
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().menu_presenter = Some(shared);
    });
    show(
        &mut harness,
        &file,
        huterm_config::settings::SectionId::Scrollback,
    );
    with_window(&mut harness, |view, window, cx| {
        window.activate_window();
        view.focus_field(path, window, cx);
    });
    keys(
        &harness,
        if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        },
    );
    let mut typing = visual(&harness);
    typing.simulate_input("5000");
    typing.run_until_parked();
    keys(
        &harness,
        if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        },
    );
    let (_, bounds) =
        with_window(&mut harness, |view, _, cx| view.field_caret(path, cx))
            .expect("the field painted");
    let mut pressing = visual(&harness);
    pressing.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: gpui::point(bounds.left() + px(2.0), bounds.center().y),
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    pressing.run_until_parked();
    harness.wait_shown(&presenter, 1);
    assert_eq!(
        presenter.shown.borrow()[0].ids(),
        [
            "text_cut",
            "text_copy",
            "text_paste",
            "-",
            "text_select_all"
        ]
    );
    assert!(!with_view(&mut harness, |view, _| view.field_menu_shown()));
    assert_eq!(focus(&mut harness), path, "the field keeps focus");
    assert!(
        harness.cx.update(settled) && file.text().is_empty(),
        "opening the menu saved"
    );
    presenter.shown.borrow()[0].pick("text_cut");
    harness.wait_for("the pick to cut the text", |cx| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref()
            == Some("5000")
    });
    assert_eq!(
        with_window(&mut harness, |view, _, cx| view.field_value(path, cx)),
        ""
    );
    assert_eq!(focus(&mut harness), path);
}

/// A Settings harness whose menus open through the returned presenter,
/// showing `section` of `file`.
fn native_settings(
    harness: &mut DesktopHarness,
    file: &super::settings::ScratchConfig,
    section: huterm_config::settings::SectionId,
) -> Rc<FakePresenter> {
    let presenter = Rc::new(FakePresenter::default());
    let shared: Rc<dyn MenuPresenter> = presenter.clone();
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().menu_presenter = Some(shared);
    });
    super::settings_controls::show(harness, file, section);
    presenter
}

/// Right-presses the first character the field shows in `bounds`.
fn right_press_field(harness: &DesktopHarness, bounds: Bounds<Pixels>) {
    let mut visual = super::settings_controls::visual(harness);
    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: gpui::point(bounds.left() + px(2.0), bounds.center().y),
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    visual.run_until_parked();
}

/// A Settings field's native menu closes when its field goes, whether its
/// section changes or its dialog closes, and a pick that arrives after
/// that edits nothing.
#[gpui::test]
fn a_settings_native_field_menu_closes_when_its_field_goes(
    cx: &mut TestAppContext,
) {
    use super::settings::{ScratchConfig, with_view, with_window};
    use huterm_config::settings::SectionId;

    let path = "terminal.scrollback_rows";
    let file = ScratchConfig::new("native-field-menu-goes", "");
    let mut harness = DesktopHarness::new(cx);
    let presenter = native_settings(&mut harness, &file, SectionId::Scrollback);
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(path, window, cx);
    });
    let (_, bounds) =
        with_window(&mut harness, |view, _, cx| view.field_caret(path, cx))
            .expect("the field painted");
    let before =
        with_window(&mut harness, |view, _, cx| view.field_value(path, cx));
    right_press_field(&harness, bounds);
    harness.wait_shown(&presenter, 1);
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::NEXT_TAB, window, cx).unwrap();
    });
    harness.cx.run_until_parked();
    assert!(presenter.shown.borrow()[0].closed.get(), "section change");
    presenter.shown.borrow()[0].pick("text_select_all");
    presenter.shown.borrow()[0].pick.close();
    harness.cx.run_until_parked();
    assert_eq!(
        with_window(&mut harness, |view, _, cx| view.field_value(path, cx)),
        before
    );
    // A dialog's field goes with its dialog.
    with_window(&mut harness, |view, window, cx| {
        view.type_profile_name("scratch", window, cx);
    });
    super::settings_controls::visual(&harness).run_until_parked();
    let (_, bounds) =
        with_window(&mut harness, |view, _, cx| view.dialog_field_caret(cx))
            .expect("the dialog's field painted");
    right_press_field(&harness, bounds);
    harness.wait_shown(&presenter, 2);
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::DIALOG_CANCEL, window, cx).unwrap();
    });
    harness.cx.run_until_parked();
    assert!(with_view(&mut harness, |view, _| view.quake_prompts()
        == (false, false)));
    assert!(presenter.shown.borrow()[1].closed.get(), "dialog close");
}

/// With native menus off, a Settings field's menu is Huterm's own even
/// where a presenter exists.
#[gpui::test]
fn turning_native_menus_off_draws_settings_field_menus(
    cx: &mut TestAppContext,
) {
    use super::settings::{ScratchConfig, with_view, with_window};
    use huterm_config::settings::SectionId;

    let path = "terminal.scrollback_rows";
    let file = ScratchConfig::new("native-field-menu-off", "");
    let mut harness = DesktopHarness::new(cx);
    let presenter = native_settings(&mut harness, &file, SectionId::Scrollback);
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().config.window.macos_native_menus = false;
    });
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(path, window, cx);
    });
    let (_, bounds) =
        with_window(&mut harness, |view, _, cx| view.field_caret(path, cx))
            .expect("the field painted");
    right_press_field(&harness, bounds);
    assert!(with_view(&mut harness, |view, _| view.field_menu_shown()));
    assert!(presenter.shown.borrow().is_empty());
}

/// Closing the palette closes its field's native menu, as a terminal
/// window's other native menus close with their target.
#[gpui::test]
fn closing_the_palette_closes_its_fields_native_menu(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _, presenter) = harness.native_window();
    harness.invoke(window, ids::OPEN_COMMAND_PALETTE).unwrap();
    harness.press(window, "a b c");
    let field = harness.cx.update(|cx| palette_field(cx, window));
    let (_, bounds) = harness
        .cx
        .update(|cx| field.read(cx).painted_caret())
        .expect("the field painted");
    harness.right_press(
        window,
        gpui::point(bounds.left() + px(1.0), bounds.center().y),
    );
    harness.wait_shown(&presenter, 1);
    harness.press(window, "escape");
    harness.wait_for("the palette and its menu to close", |cx| {
        palette_state(cx, window).is_none() && native_kind(cx, window).is_none()
    });
    assert!(presenter.shown.borrow()[0].closed.get());
}
