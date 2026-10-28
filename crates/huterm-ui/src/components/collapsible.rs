//! Collapsible: a header that shows or hides the content under it, after
//! shadcn/ui's and Radix's Collapsible, such as a keybinding command row
//! that expands to its shortcuts.
//!
//! Anatomy: a trigger, drawn from the owner's header (by default the
//! label), and the content under it while open. [`chevron`] draws the
//! usual disclosure arrow for the header, pointing down while open.
//!
//! Keyboard: the trigger takes focus; the owner toggles it on its
//! activation command (Space and Return by default). GPUI's implicit
//! keyboard click is ignored, so only that routed command acts on keys.
//!
//! Pointer: a click on the trigger toggles; hover and pressed states show.
//! A disabled collapsible takes no focus from a press, does not toggle,
//! drops its tab stop, dims its trigger, and shows its reason as a tooltip;
//! its content stays as it was.
//!
//! Accessibility: the trigger is a `Button` named by the label whose
//! expanded state follows the content, reporting a disabled reason as its
//! description. GPUI cannot name another node, so the trigger does not
//! name the content it controls.
//!
//! Reference: WAI-ARIA's disclosure pattern and Radix's Collapsible.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, FocusHandle, IntoElement, MouseButton,
    ParentElement, RenderOnce, SharedString, Svg, Window, div, prelude::*, px,
    svg,
};

use crate::a11y::States;
use crate::click::from_keyboard;
use crate::components::tooltip::Tooltip;
use crate::focus;
use crate::tokens::Tokens;

type Toggle = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A collapsible section.
#[derive(IntoElement)]
pub struct Collapsible {
    id: ElementId,
    label: SharedString,
    open: bool,
    focus: FocusHandle,
    tokens: Tokens,
    trigger: Option<AnyElement>,
    disabled: Option<SharedString>,
    key_context: Option<SharedString>,
    on_toggle: Option<Toggle>,
    content: Vec<AnyElement>,
}

impl std::fmt::Debug for Collapsible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Collapsible")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("open", &self.open)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

impl Collapsible {
    /// A section whose trigger is named `label`, `open` or closed, focused
    /// through `focus`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        open: bool,
        focus: &FocusHandle,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            open,
            focus: focus.clone(),
            tokens,
            trigger: None,
            disabled: None,
            key_context: None,
            on_toggle: None,
            content: Vec::new(),
        }
    }

    /// What the trigger draws instead of its label, such as an
    /// [`crate::components::item::Item`] with a [`chevron`].
    #[must_use]
    pub fn trigger(mut self, header: impl IntoElement) -> Self {
        self.trigger = Some(header.into_any_element());
        self
    }

    /// Disables the trigger; `reason`, when given, shows as its tooltip and
    /// accessible description.
    #[must_use]
    pub fn disabled(mut self, reason: Option<SharedString>) -> Self {
        self.disabled = Some(reason.unwrap_or_default());
        self
    }

    /// The key context the trigger carries, such as the owner's control
    /// context that its activation command binds under.
    #[must_use]
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    /// Called with the new open state for a pointer click on the trigger.
    #[must_use]
    pub fn on_toggle(
        mut self,
        toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(toggle));
        self
    }
}

impl ParentElement for Collapsible {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.content.extend(elements);
    }
}

/// The disclosure arrow drawn from the icon at `path`, which points right:
/// turned to point down while `open`.
#[must_use]
pub fn chevron(
    path: impl Into<SharedString>,
    open: bool,
    tokens: Tokens,
) -> Svg {
    svg()
        .path(path.into())
        .flex_none()
        .size(px(12.0))
        .text_color(tokens.muted)
        .when(open, |chevron| {
            chevron.with_transformation(gpui::Transformation::rotate(
                gpui::percentage(0.25),
            ))
        })
}

impl RenderOnce for Collapsible {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let open = self.open;
        let focus = self.focus.clone().tab_stop(enabled);
        let reason = self.disabled.clone().filter(|reason| !reason.is_empty());
        let header = self.trigger.unwrap_or_else(|| {
            div()
                .px(px(8.0))
                .py(px(4.0))
                .text_color(tokens.fg)
                .child(self.label.clone())
                .into_any_element()
        });
        let trigger = div()
            .id(self.id)
            .role(gpui::Role::Button)
            .aria_label(self.label)
            .aria_expanded(open)
            .when_some(reason, |trigger, reason| {
                trigger.aria_description(reason.clone()).tooltip(
                    move |_, cx| Tooltip::view(reason.clone(), tokens, cx),
                )
            })
            .track_focus(&focus)
            .when_some(self.key_context, |trigger, context| {
                trigger.key_context(context.as_ref())
            })
            .when(enabled, |trigger| {
                trigger
                    .hover(|style| style.bg(tokens.hover))
                    .active(|style| style.bg(tokens.pressed()))
            })
            .when(!enabled, |trigger| {
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
                        toggle(!open, window, cx);
                    }
                })
            })
            .child(header);
        let trigger = focus::ring(trigger, &focus, tokens, window, cx);
        let trigger = States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(trigger);
        div()
            .flex()
            .flex_col()
            .child(trigger)
            .when(open, |section| section.children(self.content))
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use gpui::{
        AccessibleAction, App, FocusHandle, KeyUpEvent, Keystroke,
        TestAppContext, VisualTestContext, Window, div, prelude::*, px,
    };

    use super::Collapsible;
    use crate::focus::ring_drawn;
    use crate::testing::{
        Activate, KEYS, a11y_action, a11y_node, bind_keys, host, tokens,
    };

    struct Probe {
        focus: FocusHandle,
        before: FocusHandle,
        toggles: Rc<RefCell<Vec<bool>>>,
    }

    /// A focusable stop, then a closed section with one line of content
    /// that toggles as an owner does: on a pointer click, and on Space
    /// routed to it while it has focus.
    fn section(
        cx: &mut TestAppContext,
        disabled: bool,
    ) -> (Probe, &mut VisualTestContext) {
        let focus = cx.update(|cx| cx.focus_handle());
        let before = cx.update(|cx| cx.focus_handle().tab_stop(true));
        let toggles = Rc::new(RefCell::new(Vec::new()));
        let open = Rc::new(Cell::new(false));
        let (drawn, drawn_before, seen) =
            (focus.clone(), before.clone(), toggles.clone());
        let (_, cx) = host(cx, move |_, _| {
            let toggle = {
                let (seen, open) = (seen.clone(), open.clone());
                move |next: bool, window: &mut Window| {
                    seen.borrow_mut().push(next);
                    open.set(next);
                    window.refresh();
                }
            };
            let section =
                Collapsible::new("copy", "Copy", open.get(), &drawn, tokens())
                    .key_context(KEYS)
                    .child(
                        div()
                            .id("shortcuts")
                            .role(gpui::Role::Label)
                            .aria_label("Shortcuts"),
                    );
            let section = if disabled {
                section.disabled(Some("The file is invalid".into()))
            } else {
                let toggle = toggle.clone();
                section.on_toggle(move |next, window, _| toggle(next, window))
            };
            let (open, routed) = (open.clone(), drawn.clone());
            div()
                .child(
                    div()
                        .id("before")
                        .track_focus(&drawn_before)
                        .size(px(10.0)),
                )
                .child(div().w(px(200.0)).child(section))
                .on_action(move |_: &Activate, window, _: &mut App| {
                    if routed.is_focused(window) {
                        toggle(!open.get(), window);
                    }
                })
                .into_any_element()
        });
        bind_keys(cx);
        (
            Probe {
                focus,
                before,
                toggles,
            },
            cx,
        )
    }

    /// Whether the content drew this frame.
    fn shows_content(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, _| {
            crate::testing::a11y_nodes(window)
                .iter()
                .any(|node| node["aria"]["label"].as_str() == Some("Shortcuts"))
        })
    }

    #[gpui::test]
    fn a_click_toggles_the_content_and_its_expanded_state(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = section(cx, false);
        cx.update(|window, _| {
            let trigger = &a11y_node(window, "copy")["aria"];
            assert_eq!(trigger["role"], "Button");
            assert_eq!(trigger["label"], "Copy");
            assert_eq!(trigger["expanded"], false);
        });
        assert!(!shows_content(cx));
        a11y_action(cx, "copy", AccessibleAction::Click);
        assert_eq!(*probe.toggles.borrow(), [true]);
        assert!(shows_content(cx));
        cx.update(|window, _| {
            assert_eq!(a11y_node(window, "copy")["aria"]["expanded"], true);
        });
        a11y_action(cx, "copy", AccessibleAction::Click);
        assert_eq!(*probe.toggles.borrow(), [true, false]);
        assert!(!shows_content(cx));
    }

    #[gpui::test]
    fn the_routed_key_toggles_and_the_implicit_key_click_does_not(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = section(cx, false);
        // Focus arrives from the keyboard, as Tab moves it.
        cx.simulate_keystrokes("a");
        cx.update(|window, cx| probe.focus.focus(window, cx));
        cx.simulate_keystrokes("space");
        assert_eq!(*probe.toggles.borrow(), [true]);
        assert!(shows_content(cx));
        // GPUI clicks a focused element on Enter's key-up; nothing routes
        // Enter here, so only that implicit click could toggle.
        cx.simulate_keystrokes("enter");
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("enter").expect("a valid keystroke"),
        });
        cx.run_until_parked();
        assert_eq!(*probe.toggles.borrow(), [true]);
        assert!(cx.update(|window, cx| ring_drawn(&probe.focus, window, cx)));
    }

    #[gpui::test]
    fn a_disabled_section_neither_toggles_nor_takes_focus(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = section(cx, true);
        a11y_action(cx, "copy", AccessibleAction::Click);
        assert!(probe.toggles.borrow().is_empty());
        assert!(!shows_content(cx));
        assert!(!cx.update(|window, _| probe.focus.is_focused(window)));
        cx.update(|window, cx| probe.before.focus(window, cx));
        cx.update(Window::focus_next);
        cx.run_until_parked();
        assert!(cx.update(|window, _| probe.before.is_focused(window)));
        cx.update(|window, _| {
            let trigger = &a11y_node(window, "copy")["aria"];
            assert_eq!(trigger["disabled"], true);
            assert_eq!(trigger["description"], "The file is invalid");
        });
    }
}
