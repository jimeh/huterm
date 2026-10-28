//! Select: a trigger showing a value, which opens a menu of the values to
//! pick from, as `NSPopUpButton` in `AppKit` and `GtkDropDown` in GTK do.
//!
//! Anatomy: the value's label and a caret in an outlined control at least
//! as wide as its `min_width`. The owner keeps the value, the open state,
//! the [`crate::components::popover::Popover`], and its
//! [`crate::components::menu::Menu`] with [`crate::components::menu::Role::ListBox`];
//! the select only draws the trigger and reports presses.
//!
//! Keyboard: the trigger takes focus; the owner opens the menu on Space,
//! Return, Down, and Up, as Radix Select does, selecting the shown value so
//! Return keeps it. While open, the menu has focus and its commands apply.
//! GPUI's implicit keyboard click is ignored, so only those routed commands
//! act on keys.
//!
//! Pointer: a press opens the menu and a press on the open trigger closes
//! it; the popover's guard turns that second press into
//! [`crate::components::popover::Dismissal::TriggerPress`]. A disabled
//! select takes no focus from a press and does not open, and shows its
//! reason as a tooltip.
//!
//! Accessibility: a `ComboBox` named by its label, whose value is the shown
//! label and whose expanded state follows the menu. A disabled select
//! reports it.

use std::rc::Rc;

use gpui::{
    App, ElementId, FocusHandle, IntoElement, MouseButton, RenderOnce,
    SharedString, Window, div, prelude::*, px,
};

use crate::a11y::States;
use crate::click::from_keyboard;
use crate::components::popover::Popover;
use crate::components::tooltip::Tooltip;
use crate::focus;
use crate::tokens::{Tokens, size};

type Toggle = Rc<dyn Fn(&mut Window, &mut App)>;

/// A select's trigger.
#[derive(IntoElement)]
pub struct Select {
    id: ElementId,
    label: SharedString,
    value: SharedString,
    focus: FocusHandle,
    tokens: Tokens,
    open: bool,
    disabled: Option<SharedString>,
    key_context: Option<SharedString>,
    min_width: gpui::Pixels,
    popover: Option<Popover>,
    on_toggle: Option<Toggle>,
}

impl std::fmt::Debug for Select {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Select")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("value", &self.value)
            .field("open", &self.open)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

impl Select {
    /// A select named `label` showing `value`, focused through `focus`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
        focus: &FocusHandle,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            focus: focus.clone(),
            tokens,
            open: false,
            disabled: None,
            key_context: None,
            min_width: px(150.0),
            popover: None,
            on_toggle: None,
        }
    }

    /// Whether the menu is open, which the trigger reports.
    #[must_use]
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Disables the select; `reason` shows as its tooltip and becomes its
    /// accessible description.
    #[must_use]
    pub fn disabled(mut self, reason: Option<SharedString>) -> Self {
        self.disabled = Some(reason.unwrap_or_default());
        self
    }

    /// The key context the trigger carries, such as the owner's control
    /// context that its open commands bind under.
    #[must_use]
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    /// The trigger's minimum width.
    #[must_use]
    pub fn min_width(mut self, width: gpui::Pixels) -> Self {
        self.min_width = width;
        self
    }

    /// The popover the trigger anchors, so its guard knows the trigger.
    /// Selects that share one popover pass it only to the trigger whose
    /// menu is open, since the trigger that paints last is the one the
    /// guard knows.
    #[must_use]
    pub fn popover(mut self, popover: &Popover) -> Self {
        self.popover = Some(popover.clone());
        self
    }

    /// Called for a pointer press, to open or close the menu.
    #[must_use]
    pub fn on_toggle(
        mut self,
        toggle: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(toggle));
        self
    }
}

impl RenderOnce for Select {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let focus = self.focus.clone().tab_stop(enabled);
        let reason = self.disabled.clone().filter(|reason| !reason.is_empty());
        let trigger = div()
            .id(self.id)
            .role(gpui::Role::ComboBox)
            .aria_label(self.label)
            .aria_value(self.value.clone())
            .aria_expanded(self.open)
            .when_some(reason, |trigger, reason| {
                trigger.aria_description(reason.clone()).tooltip(
                    move |_, cx| Tooltip::view(reason.clone(), tokens, cx),
                )
            })
            .track_focus(&focus)
            .when_some(self.key_context, |trigger, context| {
                trigger.key_context(context.as_ref())
            })
            .relative()
            .flex()
            .items_center()
            .gap(px(8.0))
            .min_w(self.min_width)
            .h(size::CONTROL_HEIGHT)
            .px(px(10.0))
            .rounded(px(size::CONTROL_RADIUS))
            .border_1()
            .border_color(tokens.line_strong)
            .bg(tokens.bg)
            .text_size(size::TEXT_CONTROL)
            .text_color(tokens.fg)
            .when(!enabled, |trigger| {
                // A disabled control keeps its handle but takes no focus
                // from a press.
                trigger.opacity(0.5).on_mouse_down(
                    MouseButton::Left,
                    |_, window, _| {
                        window.prevent_default();
                    },
                )
            })
            .when_some(self.on_toggle.filter(|_| enabled), |trigger, toggle| {
                trigger.on_click(move |event, window, cx| {
                    if !from_keyboard(event) {
                        toggle(window, cx);
                    }
                })
            })
            .child(div().flex_1().truncate().child(self.value))
            .child(div().flex_none().text_color(tokens.muted).child("▾"))
            .children(self.popover.map(|popover| popover.trigger_probe()));
        let trigger = focus::ring(trigger, &focus, tokens, window, cx);
        States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(trigger)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        FocusHandle, KeyUpEvent, Keystroke, Modifiers, TestAppContext,
        VisualTestContext, point, prelude::*, px,
    };

    use super::Select;
    use crate::focus::ring_drawn;
    use crate::testing::{a11y_node, host, tokens};

    struct Probe {
        focus: FocusHandle,
        toggles: Rc<Cell<usize>>,
    }

    fn select(
        cx: &mut TestAppContext,
        open: bool,
        disabled: bool,
    ) -> (Probe, &mut VisualTestContext) {
        let focus = cx.update(|cx| cx.focus_handle());
        let toggles = Rc::new(Cell::new(0));
        let (drawn, counted) = (focus.clone(), toggles.clone());
        let (_, cx) = host(cx, move |_, _| {
            let counted = counted.clone();
            let select =
                Select::new("theme", "Theme", "Dark", &drawn, tokens())
                    .open(open)
                    .on_toggle(move |_, _| counted.set(counted.get() + 1));
            let select = if disabled {
                select.disabled(Some("The file is invalid".into()))
            } else {
                select
            };
            select.into_any_element()
        });
        (Probe { focus, toggles }, cx)
    }

    #[gpui::test]
    fn a_select_reports_its_value_and_state(cx: &mut TestAppContext) {
        let (_, cx) = select(cx, true, false);
        cx.update(|window, _| {
            let aria = &a11y_node(window, "theme")["aria"];
            assert_eq!(aria["role"], "ComboBox");
            assert_eq!(aria["label"], "Theme");
            assert_eq!(aria["value"], "Dark");
            assert_eq!(aria["expanded"], true);
            assert!(aria["disabled"].is_null());
        });
    }

    #[gpui::test]
    fn a_pointer_press_toggles_and_an_implicit_key_click_does_not(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = select(cx, false, false);
        cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(probe.toggles.get(), 1);
        assert!(cx.update(|window, _| probe.focus.is_focused(window)));
        // GPUI clicks a focused element on Enter's key-up; nothing binds
        // Enter here, so only that implicit click could toggle.
        cx.simulate_keystrokes("enter");
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("enter").expect("a valid keystroke"),
        });
        cx.run_until_parked();
        assert_eq!(probe.toggles.get(), 1);
    }

    #[gpui::test]
    fn a_disabled_select_neither_opens_nor_takes_focus(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = select(cx, false, true);
        cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(probe.toggles.get(), 0);
        assert!(!cx.update(|window, _| probe.focus.is_focused(window)));
        cx.update(|window, _| {
            let aria = &a11y_node(window, "theme")["aria"];
            assert_eq!(aria["disabled"], true);
            assert_eq!(aria["description"], "The file is invalid");
        });
    }

    #[gpui::test]
    fn keyboard_focus_draws_the_ring(cx: &mut TestAppContext) {
        let (probe, cx) = select(cx, false, false);
        cx.simulate_keystrokes("a");
        cx.update(|window, cx| probe.focus.focus(window, cx));
        cx.run_until_parked();
        assert!(cx.update(|window, cx| ring_drawn(&probe.focus, window, cx)));
    }
}
