//! Switch: an on-or-off control whose change applies at once, such as a
//! setting, as `NSSwitch` and `GtkSwitch` are.
//!
//! Anatomy: a rounded track, the accent while on, with a knob at the end
//! that state names.
//!
//! Keyboard: the switch takes focus; the owner toggles it on its
//! activation command (Space and Return by default). Arrow keys do nothing,
//! as in WAI-ARIA's switch pattern. GPUI's implicit keyboard click is
//! ignored, so only that routed command acts on keys.
//!
//! Pointer: a click toggles; hover and pressed states show. A disabled
//! switch takes no focus from a press, does not toggle, and shows its reason
//! as a tooltip.
//!
//! Accessibility: a `Switch` named by its label whose toggled state follows
//! it, reporting a disabled reason as its description.

use std::rc::Rc;

use gpui::{
    App, ElementId, FocusHandle, Hsla, IntoElement, MouseButton, RenderOnce,
    SharedString, Toggled, Window, div, prelude::*, px,
};

use crate::a11y::States;
use crate::click::from_keyboard;
use crate::components::tooltip::Tooltip;
use crate::focus;
use crate::tokens::Tokens;

type Toggle = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A switch.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    label: SharedString,
    on: bool,
    focus: FocusHandle,
    tokens: Tokens,
    disabled: Option<SharedString>,
    key_context: Option<SharedString>,
    on_toggle: Option<Toggle>,
}

impl std::fmt::Debug for Switch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Switch")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("on", &self.on)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

impl Switch {
    /// A switch named `label`, `on` or off, focused through `focus`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        on: bool,
        focus: &FocusHandle,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            on,
            focus: focus.clone(),
            tokens,
            disabled: None,
            key_context: None,
            on_toggle: None,
        }
    }

    /// Disables the switch; `reason` becomes its accessible description.
    #[must_use]
    pub fn disabled(mut self, reason: Option<SharedString>) -> Self {
        self.disabled = Some(reason.unwrap_or_default());
        self
    }

    /// The key context the switch carries, such as the owner's control
    /// context that its toggle command binds under.
    #[must_use]
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    /// Called with the new state for a pointer click.
    #[must_use]
    pub fn on_toggle(
        mut self,
        toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(toggle));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let on = self.on;
        let focus = self.focus.clone().tab_stop(enabled);
        let reason = self.disabled.clone().filter(|reason| !reason.is_empty());
        // The off track alone shows the state, so it keeps 3:1.
        let off = tokens.state_line();
        let deeper = |amount: f32| Hsla {
            a: (off.a + amount).min(1.0),
            ..off
        };
        let (track, hover, pressed) = if on {
            (
                tokens.accent,
                tokens.shade(tokens.accent, 0.1),
                tokens.accent_pressed(),
            )
        } else {
            (off, deeper(0.08), deeper(0.16))
        };
        let switch = div()
            .id(self.id)
            .role(gpui::Role::Switch)
            .aria_label(self.label)
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .when_some(reason, |switch, reason| {
                switch
                    .aria_description(reason.clone())
                    .tooltip(move |_, cx| {
                        Tooltip::view(reason.clone(), tokens, cx)
                    })
            })
            .track_focus(&focus)
            .when_some(self.key_context, |switch, context| {
                switch.key_context(context.as_ref())
            })
            .flex_none()
            .w(px(34.0))
            .h(px(20.0))
            .p(px(2.0))
            .rounded_full()
            .bg(track)
            .when(enabled, |switch| {
                switch
                    .hover(|style| style.bg(hover))
                    .active(|style| style.bg(pressed))
            })
            .when(!enabled, |switch| {
                switch.opacity(0.5).on_mouse_down(
                    MouseButton::Left,
                    |_, window, _| {
                        window.prevent_default();
                    },
                )
            })
            .when_some(self.on_toggle.filter(|_| enabled), |switch, toggle| {
                switch.on_click(move |event, window, cx| {
                    if !from_keyboard(event) {
                        toggle(!on, window, cx);
                    }
                })
            })
            .child(
                div()
                    .size(px(16.0))
                    .rounded_full()
                    // On the accent, the knob takes the accent's text color
                    // so it stands out; off, the background.
                    .bg(if on { tokens.on_accent() } else { tokens.bg })
                    .when(on, |knob| knob.ml(px(14.0))),
            );
        let switch = focus::ring(switch, &focus, tokens, window, cx);
        States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(switch)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{
        FocusHandle, KeyUpEvent, Keystroke, Modifiers, TestAppContext,
        VisualTestContext, point, prelude::*, px,
    };

    use super::Switch;
    use crate::focus::ring_drawn;
    use crate::testing::{a11y_node, host, tokens};

    struct Probe {
        focus: FocusHandle,
        toggles: Rc<RefCell<Vec<bool>>>,
    }

    fn switch(
        cx: &mut TestAppContext,
        on: bool,
        disabled: bool,
    ) -> (Probe, &mut VisualTestContext) {
        let focus = cx.update(|cx| cx.focus_handle());
        let toggles = Rc::new(RefCell::new(Vec::new()));
        let (drawn, seen) = (focus.clone(), toggles.clone());
        let (_, cx) = host(cx, move |_, _| {
            let seen = seen.clone();
            let switch =
                Switch::new("bell", "Audible bell", on, &drawn, tokens())
                    .on_toggle(move |on, _, _| seen.borrow_mut().push(on));
            let switch = if disabled {
                switch.disabled(Some("The file is invalid".into()))
            } else {
                switch
            };
            switch.into_any_element()
        });
        (Probe { focus, toggles }, cx)
    }

    #[gpui::test]
    fn a_switch_reports_its_state(cx: &mut TestAppContext) {
        let (_, cx) = switch(cx, true, false);
        cx.update(|window, _| {
            let aria = &a11y_node(window, "bell")["aria"];
            assert_eq!(aria["role"], "Switch");
            assert_eq!(aria["label"], "Audible bell");
            assert_eq!(aria["toggled"], "True");
        });
    }

    #[gpui::test]
    fn a_pointer_click_toggles_and_an_implicit_key_click_does_not(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = switch(cx, false, false);
        cx.simulate_click(point(px(10.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(*probe.toggles.borrow(), [true]);
        assert!(cx.update(|window, _| probe.focus.is_focused(window)));
        cx.simulate_keystrokes("enter");
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("enter").expect("a valid keystroke"),
        });
        cx.run_until_parked();
        assert_eq!(*probe.toggles.borrow(), [true]);
    }

    #[gpui::test]
    fn a_disabled_switch_neither_toggles_nor_takes_focus(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = switch(cx, false, true);
        cx.simulate_click(point(px(10.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert!(probe.toggles.borrow().is_empty());
        assert!(!cx.update(|window, _| probe.focus.is_focused(window)));
        cx.update(|window, _| {
            let aria = &a11y_node(window, "bell")["aria"];
            assert_eq!(aria["disabled"], true);
            assert_eq!(aria["description"], "The file is invalid");
        });
    }

    #[gpui::test]
    fn the_ring_shows_for_keyboard_focus_until_a_press(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = switch(cx, false, false);
        cx.simulate_keystrokes("a");
        cx.update(|window, cx| probe.focus.focus(window, cx));
        cx.run_until_parked();
        let ring = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| ring_drawn(&probe.focus, window, cx))
        };
        assert!(ring(cx));
        // Pointer motion alone keeps it; a press on the switch hides it.
        cx.simulate_mouse_move(
            point(px(10.0), px(10.0)),
            None,
            Modifiers::none(),
        );
        cx.run_until_parked();
        assert!(ring(cx));
        cx.simulate_click(point(px(10.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert!(!ring(cx));
    }
}
