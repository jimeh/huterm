//! The dialog contract tests, and the host the alert dialog's tests share.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AccessibleAction, Context, DispatchPhase, Entity, FocusHandle, Focusable,
    IntoElement, KeyBinding, KeyUpEvent, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, Pixels, Point, Render, ScrollDelta, ScrollEvent,
    TestAppContext, TouchPhase, VisualTestContext, Window, canvas, div, point,
    prelude::*, px,
};

use super::{Dialog, Dismissal, KEY_CONTEXT, Modal};
use crate::components::alert_dialog::AlertDialog;
use crate::components::button::{Button, Variant};
use crate::components::input::{Chrome, Input};
use crate::components::input_group::InputGroup;
use crate::testing::{a11y_action, a11y_node, tokens};

gpui::actions!(
    dialog_test,
    [
        Next,
        Previous,
        Cancel,
        MoveLeft,
        MoveRight,
        FieldNext,
        FieldPrevious,
        FieldBack
    ]
);

/// The rename field's own key context, standing in for Huterm's `Palette`,
/// which its text fields carry.
const FIELD_CONTEXT: &str = "Palette";

/// Which dialog the host draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A rename dialog: a field in an input group, an inline error, and
    /// Cancel and Rename buttons.
    Rename,
    /// The rename dialog opening on Rename rather than the field.
    RenameOnButton,
    /// The rename dialog with a body taller than the host.
    RenameTall,
    /// A delete confirmation with Cancel and Delete, the action added
    /// first.
    Alert,
    /// The delete confirmation opening on Delete rather than Cancel.
    AlertOnAction,
    /// A bare rename dialog: the owner's own heading and field, then the
    /// footer's Cancel and Rename.
    Bare,
    /// The rename dialog in an area [`INSET`] points inside the host's
    /// edges, as a client-drawn frame's content is, with an edge strip in
    /// the margin below it, as a frame's resize edge is.
    RenameInset,
}

/// How far [`Kind::RenameInset`]'s dialog area sits inside the host.
const INSET: Pixels = px(40.0);

type Log = Rc<RefCell<Vec<String>>>;

/// A way to dismiss the host's open dialog.
type Dismiss = fn(&Probe, &mut VisualTestContext);

/// An owner with a focusable trigger at the top left, beneath the scrim
/// once the dialog opens, and a window-level press and wheel listener that
/// stands in for a terminal's. It routes Tab, Shift-Tab, Escape, Left, and
/// Right under the dialog's key context, as Huterm's defaults bind them,
/// and Left, Right, Tab, Shift-Tab, and Escape under the field's, closes on
/// every dismissal reason except those it refuses, and logs what it sees.
pub(crate) struct Owner {
    modal: Modal,
    kind: Kind,
    refuse: Vec<Dismissal>,
    trigger: FocusHandle,
    field: Entity<Input>,
    cancel: FocusHandle,
    confirm: FocusHandle,
    /// The footer buttons drawn disabled, by name.
    disabled: Vec<&'static str>,
    log: Log,
}

impl Owner {
    fn open(&self) {
        self.modal.open(&self.trigger);
    }

    fn dismiss(
        &mut self,
        reason: Dismissal,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.log.borrow_mut().push(format!("{reason:?}"));
        if !self.refuse.contains(&reason) {
            let _ = self.modal.close(window, cx);
        }
        cx.notify();
    }

    fn dialog(&self, cx: &mut Context<'_, Self>) -> gpui::AnyElement {
        let entry = |text: &'static str| {
            let log = self.log.clone();
            move || log.borrow_mut().push(text.to_owned())
        };
        let cancel_click = entry("cancel-click");
        let confirm_click = entry("confirm-click");
        let view = cx.entity().downgrade();
        let disable = |button: Button, name: &str| {
            if self.disabled.contains(&name) {
                button.disabled(None)
            } else {
                button
            }
        };
        let cancel = Button::new("cancel", "Cancel", tokens())
            .debug_selector(|| "cancel".to_owned())
            .on_click(move |window, cx| {
                cancel_click();
                let _ = view.update(cx, |owner, cx| {
                    let _ = owner.modal.close(window, cx);
                    cx.notify();
                });
            });
        let cancel = disable(cancel, "cancel");
        if matches!(self.kind, Kind::Alert | Kind::AlertOnAction) {
            let confirm = Button::new("confirm", "Delete", tokens())
                .variant(Variant::Destructive)
                .on_click(move |_, _| confirm_click());
            let confirm = disable(confirm, "confirm");
            return AlertDialog::new(
                "dialog",
                "Delete profile?",
                &self.modal,
                tokens(),
            )
            .description("This cannot be undone.")
            .action(confirm, &self.confirm)
            .cancel(cancel, &self.cancel)
            .when(self.kind == Kind::AlertOnAction, |alert| {
                alert.initial_focus(&self.confirm)
            })
            .into_any_element();
        }
        let confirm = Button::new("confirm", "Rename", tokens())
            .variant(Variant::Primary)
            .on_click(move |_, _| confirm_click());
        let confirm = disable(confirm, "confirm");
        if self.kind == Kind::Bare {
            return Dialog::new(
                "dialog",
                "Rename profile",
                &self.modal,
                tokens(),
            )
            .description("Profiles are listed by name.")
            .bare()
            .focusable(&self.field.focus_handle(cx))
            .child(
                div()
                    .debug_selector(|| "content".to_owned())
                    .child("Rename profile")
                    .child(div().key_context(FIELD_CONTEXT).child(
                        InputGroup::new("group", &self.field, tokens()),
                    )),
            )
            .button(cancel, &self.cancel)
            .button(confirm, &self.confirm)
            .into_any_element();
        }
        Dialog::new("dialog", "Rename profile", &self.modal, tokens())
            .description("Profiles are listed by name.")
            .focusable(&self.field.focus_handle(cx))
            .child(div().key_context(FIELD_CONTEXT).child(InputGroup::new(
                "group",
                &self.field,
                tokens(),
            )))
            .child(div().child("A profile with that name exists."))
            .when(self.kind == Kind::RenameTall, |dialog| {
                dialog.child(div().flex_none().h(px(1000.0)))
            })
            .button(cancel, &self.cancel)
            .button(confirm, &self.confirm)
            .when(self.kind == Kind::RenameOnButton, |dialog| {
                dialog.initial_focus(&self.confirm)
            })
            .into_any_element()
    }

    /// The dialog in the area it covers: the host, or for
    /// [`Kind::RenameInset`], the area inside the edge strip's margin.
    fn mounted(&self, cx: &mut Context<'_, Self>) -> gpui::AnyElement {
        let dialog = self.dialog(cx);
        if self.kind == Kind::RenameInset {
            div()
                .absolute()
                .inset(INSET)
                .child(dialog)
                .into_any_element()
        } else {
            dialog
        }
    }

    /// [`Kind::RenameInset`]'s edge strip along the host's bottom, in the
    /// margin outside the dialog's area.
    fn edge(&self) -> impl IntoElement {
        let (down, wheel) = (self.log.clone(), self.log.clone());
        div()
            .id("edge")
            .absolute()
            .left_0()
            .bottom_0()
            .w_full()
            .h(INSET)
            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                down.borrow_mut().push("edge-down".to_owned());
            })
            .on_scroll(move |_, _, _| {
                wheel.borrow_mut().push("edge-wheel".to_owned());
            })
    }
}

impl Render for Owner {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let entry = |text: &'static str| {
            let log = self.log.clone();
            move || log.borrow_mut().push(text.to_owned())
        };
        let (trigger_down, trigger_wheel) =
            (entry("trigger-down"), entry("trigger-wheel"));
        let (window_down, window_wheel) =
            (entry("window-down"), entry("window-wheel"));
        let view = cx.entity().downgrade();
        let terminal = canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                let window_down = window_down.clone();
                window.on_mouse_event(
                    move |_: &MouseDownEvent, phase, _, _| {
                        if phase == DispatchPhase::Bubble {
                            window_down();
                        }
                    },
                );
                let window_wheel = window_wheel.clone();
                window.on_mouse_event(move |_: &ScrollEvent, phase, _, _| {
                    if phase == DispatchPhase::Bubble {
                        window_wheel();
                    }
                });
            },
        )
        .absolute()
        .size(px(0.0));
        div()
            .size(px(400.0))
            .relative()
            .on_action(cx.listener(|owner, _: &Next, window, cx| {
                owner.modal.focus_next(window, cx);
            }))
            .on_action(cx.listener(|owner, _: &Previous, window, cx| {
                owner.modal.focus_previous(window, cx);
            }))
            .on_action(cx.listener(|owner, _: &Cancel, window, cx| {
                owner.dismiss(Dismissal::Escape, window, cx);
            }))
            .on_action(cx.listener(|owner, _: &MoveLeft, _, cx| {
                owner
                    .field
                    .update(cx, |field, cx| field.move_left(false, cx));
            }))
            .on_action(cx.listener(|owner, _: &MoveRight, _, cx| {
                owner
                    .field
                    .update(cx, |field, cx| field.move_right(false, cx));
            }))
            .on_action(cx.listener(|owner, _: &FieldNext, window, cx| {
                owner.modal.focus_next(window, cx);
            }))
            .on_action(cx.listener(|owner, _: &FieldPrevious, window, cx| {
                owner.modal.focus_previous(window, cx);
            }))
            .on_action(cx.listener(|owner, _: &FieldBack, window, cx| {
                owner.dismiss(Dismissal::Escape, window, cx);
            }))
            .child(crate::focus::tracker())
            .child(terminal)
            .child(self.modal.guard(move |reason, window, cx| {
                let _ = view.update(cx, |owner, cx| {
                    owner.dismiss(reason, window, cx);
                });
            }))
            .child(
                div()
                    .id("trigger")
                    .track_focus(&self.trigger)
                    .absolute()
                    .w(px(100.0))
                    .h(px(24.0))
                    .on_mouse_down(MouseButton::Left, move |_, _, _| {
                        trigger_down();
                    })
                    .on_scroll(move |_, _, _| trigger_wheel())
                    .on_click(cx.listener(|owner, _, _, cx| {
                        owner.open();
                        cx.notify();
                    })),
            )
            .when(self.kind == Kind::RenameInset, |root| {
                root.child(self.edge())
            })
            .when(self.modal.is_open(), |root| root.child(self.mounted(cx)))
    }
}

/// A host window and the handles tests read.
pub(crate) struct Probe {
    owner: Entity<Owner>,
    log: Log,
    trigger: FocusHandle,
    field: FocusHandle,
    cancel: FocusHandle,
    confirm: FocusHandle,
}

impl Probe {
    /// Opens the dialog with a click on its trigger, as a user does, and
    /// draws it.
    pub(crate) fn open(&self, cx: &mut VisualTestContext) {
        click(cx, beneath());
        assert!(self.is_open(cx), "the trigger opens the dialog");
        // The opening press reached the trigger and the window listener.
        let _ = self.log();
    }

    pub(crate) fn is_open(&self, cx: &mut VisualTestContext) -> bool {
        self.owner.read_with(cx, |owner, _| owner.modal.is_open())
    }

    /// What the host logged since the last call.
    pub(crate) fn log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.borrow_mut())
    }

    /// Draws the footer buttons named in `names` disabled, and the others
    /// enabled.
    pub(crate) fn disable(
        &self,
        names: &[&'static str],
        cx: &mut VisualTestContext,
    ) {
        self.owner.update(cx, |owner, cx| {
            owner.disabled = names.to_vec();
            cx.notify();
        });
        cx.run_until_parked();
    }

    fn field_text(&self, cx: &mut VisualTestContext) -> String {
        self.owner
            .read_with(cx, |owner, cx| owner.field.read(cx).text().to_owned())
    }

    fn dismiss(&self, reason: Dismissal, cx: &mut VisualTestContext) {
        let owner = self.owner.clone();
        cx.update(|window, cx| {
            owner.update(cx, |owner, cx| owner.dismiss(reason, window, cx));
        });
        cx.run_until_parked();
    }
}

/// Opens a host window drawing `kind`, refusing the `refuse` reasons, with
/// the dialog's and the field's keys bound as [`Owner`] describes.
pub(crate) fn owner<'a>(
    cx: &'a mut TestAppContext,
    kind: Kind,
    refuse: &[Dismissal],
) -> (Probe, &'a mut VisualTestContext) {
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new("tab", Next, Some(KEY_CONTEXT)),
            KeyBinding::new("shift-tab", Previous, Some(KEY_CONTEXT)),
            KeyBinding::new("escape", Cancel, Some(KEY_CONTEXT)),
            KeyBinding::new("right", Next, Some(KEY_CONTEXT)),
            KeyBinding::new("left", Previous, Some(KEY_CONTEXT)),
            KeyBinding::new("left", MoveLeft, Some(FIELD_CONTEXT)),
            KeyBinding::new("right", MoveRight, Some(FIELD_CONTEXT)),
            KeyBinding::new("tab", FieldNext, Some(FIELD_CONTEXT)),
            KeyBinding::new("shift-tab", FieldPrevious, Some(FIELD_CONTEXT)),
            KeyBinding::new("escape", FieldBack, Some(FIELD_CONTEXT)),
        ]);
    });
    let log = Log::default();
    let shared = log.clone();
    let refuse = refuse.to_vec();
    let (owner, cx) = cx.add_window_view(move |window, cx| {
        cx.observe_window_activation(
            window,
            |owner: &mut Owner, window, cx| {
                if !window.is_window_active() && owner.modal.is_open() {
                    owner.dismiss(Dismissal::Deactivated, window, cx);
                }
            },
        )
        .detach();
        Owner {
            modal: Modal::new(),
            kind,
            refuse,
            trigger: cx.focus_handle(),
            field: cx.new(|cx| {
                Input::new("field", "Name", tokens(), cx)
                    .with_chrome(Chrome::Grouped)
            }),
            cancel: cx.focus_handle(),
            confirm: cx.focus_handle(),
            disabled: Vec::new(),
            log: shared,
        }
    });
    cx.run_until_parked();
    let probe = owner.read_with(cx, |view, cx| Probe {
        owner: owner.clone(),
        log,
        trigger: view.trigger.clone(),
        field: view.field.focus_handle(cx),
        cancel: view.cancel.clone(),
        confirm: view.confirm.clone(),
    });
    (probe, cx)
}

/// A point on the host's trigger, which the open dialog's scrim covers.
pub(crate) fn beneath() -> Point<Pixels> {
    point(px(50.0), px(12.0))
}

/// The name of the host control with focus, if any.
pub(crate) fn focused(
    probe: &Probe,
    cx: &mut VisualTestContext,
) -> Option<&'static str> {
    cx.update(|window, _| {
        [
            ("trigger", &probe.trigger),
            ("field", &probe.field),
            ("cancel", &probe.cancel),
            ("confirm", &probe.confirm),
        ]
        .into_iter()
        .find(|(_, handle)| handle.is_focused(window))
        .map(|(name, _)| name)
    })
}

pub(crate) fn click(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_click(position, Modifiers::none());
    cx.run_until_parked();
}

/// Tab, or Shift-Tab when `back`.
pub(crate) fn press_tab(cx: &mut VisualTestContext, back: bool) {
    cx.simulate_keystrokes(if back { "shift-tab" } else { "tab" });
    cx.run_until_parked();
}

fn rename(cx: &mut TestAppContext) -> (Probe, &mut VisualTestContext) {
    owner(cx, Kind::Rename, &[])
}

/// A point on [`Kind::RenameInset`]'s edge strip, outside the scrim.
fn on_the_edge() -> Point<Pixels> {
    point(px(200.0), px(380.0))
}

/// Opens [`Kind::RenameInset`] and checks that its scrim painted where its
/// area is, inside the edge strip.
fn open_inset(probe: &Probe, cx: &mut VisualTestContext) {
    probe.open(cx);
    let scrim = probe
        .owner
        .read_with(cx, |owner, _| owner.modal.state.borrow().scrim)
        .expect("the scrim painted");
    assert_eq!(scrim.origin, point(INSET, INSET));
    assert!(!scrim.contains(&on_the_edge()));
}

/// A wheel tick over `position`.
fn wheel(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_event(ScrollEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.0), px(-20.0))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        ..Default::default()
    });
    cx.run_until_parked();
}

#[gpui::test]
fn opening_focuses_the_first_control(cx: &mut TestAppContext) {
    let (probe, cx) = rename(cx);
    probe.open(cx);
    assert_eq!(focused(&probe, cx), Some("field"));
}

#[gpui::test]
fn opening_focuses_the_control_the_owner_chose(cx: &mut TestAppContext) {
    let (probe, cx) = owner(cx, Kind::RenameOnButton, &[]);
    probe.open(cx);
    assert_eq!(focused(&probe, cx), Some("confirm"));
}

#[gpui::test]
fn tab_and_shift_tab_cycle_within_the_dialog(cx: &mut TestAppContext) {
    let (probe, cx) = rename(cx);
    probe.open(cx);
    let mut seen = Vec::new();
    for _ in 0..4 {
        press_tab(cx, false);
        seen.push(focused(&probe, cx));
    }
    assert_eq!(
        seen,
        [
            Some("cancel"),
            Some("confirm"),
            Some("field"),
            Some("cancel")
        ]
    );
    seen.clear();
    for _ in 0..4 {
        press_tab(cx, true);
        seen.push(focused(&probe, cx));
    }
    assert_eq!(
        seen,
        [
            Some("field"),
            Some("confirm"),
            Some("cancel"),
            Some("field")
        ]
    );
}

/// The name of the control the host's modal remembers as last focused.
fn last_focused(
    probe: &Probe,
    cx: &mut VisualTestContext,
) -> Option<&'static str> {
    let last = probe
        .owner
        .read_with(cx, |owner, _| owner.modal.last_focused())?;
    [
        ("trigger", &probe.trigger),
        ("field", &probe.field),
        ("cancel", &probe.cancel),
        ("confirm", &probe.confirm),
    ]
    .into_iter()
    .find(|(_, handle)| **handle == last)
    .map(|(name, _)| name)
}

#[gpui::test]
fn the_modal_remembers_the_control_that_last_held_focus(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = rename(cx);
    probe.open(cx);
    press_tab(cx, false);
    assert_eq!(focused(&probe, cx), Some("cancel"));
    // Focus elsewhere in the window, as on a toast, leaves it remembered.
    let trigger = probe.trigger.clone();
    cx.update(|window, cx| trigger.focus(window, cx));
    cx.run_until_parked();
    assert_eq!(focused(&probe, cx), Some("trigger"));
    assert_eq!(last_focused(&probe, cx), Some("cancel"));
    // A control focused other than by Tab, as by a click, is remembered
    // from the frame that draws it focused.
    let confirm = probe.confirm.clone();
    cx.update(|window, cx| confirm.focus(window, cx));
    cx.run_until_parked();
    assert_eq!(last_focused(&probe, cx), Some("confirm"));
    // A control that leaves the dialog is not offered.
    probe.disable(&["confirm"], cx);
    assert_eq!(last_focused(&probe, cx), None);
    probe.disable(&[], cx);
    press_tab(cx, false);
    assert_eq!(last_focused(&probe, cx), Some("field"));
    probe.dismiss(Dismissal::Escape, cx);
    assert_eq!(last_focused(&probe, cx), None);
}

#[gpui::test]
fn every_dismissal_returns_focus_to_the_trigger(cx: &mut TestAppContext) {
    let (probe, cx) = rename(cx);
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let dismissals: [(&str, Dismiss); 5] = [
        ("Escape", |_, cx| {
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
        }),
        ("OutsidePress", |_, cx| click(cx, beneath())),
        ("Deactivated", |_, cx| cx.deactivate_window()),
        ("OwnerChanged", |probe, cx| {
            probe.dismiss(Dismissal::OwnerChanged, cx);
        }),
        ("cancel-click", |_, cx| {
            a11y_action(cx, "cancel", AccessibleAction::Click);
        }),
    ];
    for (reason, dismiss) in dismissals {
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        probe.open(cx);
        assert_eq!(focused(&probe, cx), Some("field"), "{reason}");
        dismiss(&probe, cx);
        let log = probe.log();
        assert!(log.iter().any(|entry| entry == reason), "{reason}: {log:?}");
        assert!(!log.contains(&"trigger-down".to_owned()), "{log:?}");
        assert!(!probe.is_open(cx), "{reason} closes");
        assert_eq!(focused(&probe, cx), Some("trigger"), "{reason}");
    }
}

#[gpui::test]
fn an_owner_can_keep_the_dialog_open(cx: &mut TestAppContext) {
    let refused = [Dismissal::OutsidePress, Dismissal::Deactivated];
    let (probe, cx) = owner(cx, Kind::Rename, &refused);
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    probe.open(cx);
    click(cx, beneath());
    cx.deactivate_window();
    assert_eq!(probe.log(), ["OutsidePress", "Deactivated"]);
    assert!(probe.is_open(cx));
    assert_eq!(focused(&probe, cx), Some("field"));
}

#[gpui::test]
fn presses_and_wheel_events_beneath_the_scrim_stop(cx: &mut TestAppContext) {
    let (probe, cx) = owner(cx, Kind::Rename, &[Dismissal::OutsidePress]);
    probe.open(cx);
    click(cx, beneath());
    wheel(cx, beneath());
    // Only the owner hears of the press; neither the trigger beneath nor a
    // window-level listener does.
    assert_eq!(probe.log(), ["OutsidePress"]);
    assert_eq!(focused(&probe, cx), Some("field"));
    // A press and a wheel tick inside the panel still reach it.
    let panel = probe
        .owner
        .read_with(cx, |owner, _| owner.modal.panel_bounds())
        .expect("the panel painted");
    let inside = panel.center();
    click(cx, inside);
    wheel(cx, inside);
    assert_eq!(probe.log(), ["window-down", "window-wheel"]);
}

#[gpui::test]
fn a_press_before_the_first_frame_is_stopped(cx: &mut TestAppContext) {
    let (probe, cx) = rename(cx);
    cx.update(|window, cx| probe.trigger.focus(window, cx));
    cx.run_until_parked();
    // Open without drawing: the last frame has no scrim. A simulated press
    // draws a window its owner notified first, so the notice follows it.
    probe.owner.update(cx, |owner, _| owner.open());
    cx.simulate_mouse_down(beneath(), MouseButton::Left, Modifiers::none());
    assert_eq!(probe.log(), Vec::<String>::new(), "nothing beneath acts");
    probe.owner.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    cx.simulate_mouse_up(beneath(), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert!(
        probe.is_open(cx),
        "a press before the first frame dismisses nothing"
    );
    assert_eq!(probe.log(), Vec::<String>::new());
    assert_eq!(focused(&probe, cx), Some("field"));
}

#[gpui::test]
fn a_dialog_reports_its_role_name_and_description(cx: &mut TestAppContext) {
    let (probe, cx) = rename(cx);
    probe.open(cx);
    cx.update(|window, _| {
        let aria = &a11y_node(window, "dialog")["aria"];
        assert_eq!(aria["role"], "Dialog");
        assert_eq!(aria["label"], "Rename profile");
        assert_eq!(aria["description"], "Profiles are listed by name.");
        assert_eq!(aria["modal"], true);
    });
}

#[gpui::test]
fn an_implicit_key_click_on_a_dialog_button_does_nothing(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = rename(cx);
    probe.open(cx);
    press_tab(cx, false);
    assert_eq!(focused(&probe, cx), Some("cancel"));
    // GPUI clicks a focused element on Enter's or Space's key-up; nothing
    // binds them here, so only that implicit click could act.
    for key in ["enter", "space"] {
        cx.simulate_keystrokes(key);
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("a valid keystroke"),
        });
        cx.run_until_parked();
    }
    assert_eq!(probe.log(), Vec::<String>::new());
    assert!(probe.is_open(cx));
}

#[gpui::test]
fn a_tall_body_keeps_the_footer_buttons_pressable(cx: &mut TestAppContext) {
    // The body is taller than the 400-point host.
    let (probe, cx) = owner(cx, Kind::RenameTall, &[]);
    probe.open(cx);
    let cancel = cx.debug_bounds("cancel").expect("Cancel drew");
    click(cx, cancel.center());
    assert_eq!(probe.log(), ["window-down", "cancel-click"]);
    assert!(!probe.is_open(cx), "a pointer press on Cancel closes");
    probe.open(cx);
    a11y_action(cx, "cancel", AccessibleAction::Click);
    assert_eq!(probe.log(), ["window-down", "cancel-click"]);
    assert!(
        !probe.is_open(cx),
        "an accessibility click on Cancel closes"
    );
}

#[gpui::test]
fn a_field_in_the_dialog_keeps_its_editing_keys(cx: &mut TestAppContext) {
    let (probe, cx) = rename(cx);
    probe.open(cx);
    assert_eq!(focused(&probe, cx), Some("field"));
    cx.simulate_input("ab");
    // Left and Right move the caret rather than the dialog's focus.
    cx.simulate_keystrokes("left");
    cx.simulate_input("X");
    cx.simulate_keystrokes("right");
    cx.simulate_input("Y");
    cx.run_until_parked();
    assert_eq!(probe.field_text(cx), "aXbY");
    assert_eq!(focused(&probe, cx), Some("field"));
    // The field's own Tab still moves through the dialog.
    press_tab(cx, false);
    assert_eq!(focused(&probe, cx), Some("cancel"));
    press_tab(cx, true);
    assert_eq!(focused(&probe, cx), Some("field"));
    press_tab(cx, true);
    assert_eq!(focused(&probe, cx), Some("confirm"));
}

#[gpui::test]
fn tab_skips_a_disabled_button_until_it_is_enabled(cx: &mut TestAppContext) {
    let (probe, cx) = rename(cx);
    probe.disable(&["confirm"], cx);
    probe.open(cx);
    press_tab(cx, false);
    assert_eq!(focused(&probe, cx), Some("cancel"));
    press_tab(cx, false);
    assert_eq!(focused(&probe, cx), Some("field"), "Rename is skipped");
    probe.disable(&[], cx);
    press_tab(cx, false);
    press_tab(cx, false);
    assert_eq!(focused(&probe, cx), Some("confirm"), "Rename is back");
}

#[gpui::test]
fn opening_skips_a_disabled_first_button(cx: &mut TestAppContext) {
    // The alert opens on its first button, Cancel, unless it is disabled.
    let (probe, cx) = owner(cx, Kind::Alert, &[]);
    probe.disable(&["cancel"], cx);
    probe.open(cx);
    assert_eq!(focused(&probe, cx), Some("confirm"));
}

#[gpui::test]
fn focus_enters_an_alert_whose_controls_are_all_disabled(
    cx: &mut TestAppContext,
) {
    // Keys must not reach what the scrim covers.
    let (probe, cx) = owner(cx, Kind::Alert, &[]);
    probe.disable(&["cancel", "confirm"], cx);
    probe.open(cx);
    assert_eq!(focused(&probe, cx), Some("cancel"));
}

#[gpui::test]
fn a_bare_dialog_draws_only_the_owners_content(cx: &mut TestAppContext) {
    let (probe, cx) = owner(cx, Kind::Bare, &[]);
    probe.open(cx);
    let panel = probe
        .owner
        .read_with(cx, |owner, _| owner.modal.panel_bounds())
        .expect("the panel painted");
    let content = cx.debug_bounds("content").expect("the content drew");
    // The panel's bounds are inside its border, where the content starts:
    // no title and no body padding.
    assert_eq!(content.origin, panel.origin);
    assert_eq!(content.size.width, panel.size.width);
    assert!(cx.debug_bounds("cancel").is_some(), "the footer drew");
    assert_eq!(focused(&probe, cx), Some("field"));
    cx.update(|window, _| {
        let aria = &a11y_node(window, "dialog")["aria"];
        assert_eq!(aria["role"], "Dialog");
        assert_eq!(aria["label"], "Rename profile");
        assert_eq!(aria["description"], "Profiles are listed by name.");
        assert_eq!(aria["modal"], true);
    });
}

#[gpui::test]
fn opening_skips_a_disabled_chosen_button(cx: &mut TestAppContext) {
    let (probe, cx) = owner(cx, Kind::RenameOnButton, &[]);
    probe.disable(&["confirm"], cx);
    probe.open(cx);
    assert_eq!(focused(&probe, cx), Some("field"));
}

#[gpui::test]
fn a_press_outside_the_scrim_reaches_what_is_there(cx: &mut TestAppContext) {
    let (probe, cx) = owner(cx, Kind::RenameInset, &[]);
    open_inset(&probe, cx);
    click(cx, on_the_edge());
    // The dialog has no say: it hears of no press and stays open.
    assert_eq!(probe.log(), ["edge-down", "window-down"]);
    assert!(probe.is_open(cx));
}

#[gpui::test]
fn a_press_on_an_inset_scrim_is_stopped_and_reported(cx: &mut TestAppContext) {
    let (probe, cx) = owner(cx, Kind::RenameInset, &[Dismissal::OutsidePress]);
    open_inset(&probe, cx);
    let panel = probe
        .owner
        .read_with(cx, |owner, _| owner.modal.panel_bounds())
        .expect("the panel painted");
    // On the scrim's margin, beside the panel.
    let on_scrim = point(INSET + px(8.0), panel.center().y);
    assert!(!panel.contains(&on_scrim));
    click(cx, on_scrim);
    assert_eq!(probe.log(), ["OutsidePress"]);
    assert_eq!(focused(&probe, cx), Some("field"));
}

#[gpui::test]
fn a_wheel_event_outside_the_scrim_reaches_what_is_there(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = owner(cx, Kind::RenameInset, &[]);
    open_inset(&probe, cx);
    wheel(cx, on_the_edge());
    assert_eq!(probe.log(), ["edge-wheel", "window-wheel"]);
}
