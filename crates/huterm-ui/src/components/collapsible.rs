//! Collapsible: a header that shows or hides the content under it, after
//! shadcn/ui's and Radix's Collapsible, such as a keybinding command row
//! that expands to its shortcuts.
//!
//! Anatomy: a header, and the content under it while open. The header is
//! the trigger, drawing the label, or an
//! [`crate::components::item::Item`] given through [`Collapsible::item`]:
//! the trigger then covers the item's media, title, and description, and
//! the item's actions, such as key caps and buttons, sit beside the
//! trigger, outside it. [`chevron`] draws the usual disclosure arrow for
//! the item's media, pointing down while open.
//!
//! Keyboard: the trigger takes focus; the owner toggles it on its
//! activation command (Space and Return by default) with the state
//! [`toggled`] returns. GPUI's implicit keyboard click is ignored, so only
//! that routed command acts on keys.
//!
//! Pointer: a click on the trigger toggles; hover and pressed states show.
//! A press on one of the item's actions runs that action alone. A disabled
//! collapsible takes no focus from a press, does not toggle, drops its tab
//! stop, refuses its routed command ([`toggled`] takes the owner's live
//! enabled state), dims its trigger, and shows its reason as a tooltip;
//! its content stays as it was.
//!
//! Focus: the trigger is a full-width row, so it draws the focus ring
//! inside its edge ([`crate::focus::ring_inset`]), where a card or a
//! scrolled list that clips its overflow cannot hide it. Its fill and ring
//! round to the control radius, or to [`Collapsible::corners`], such as
//! [`crate::components::card::row_corners`] for a row of a card.
//!
//! Accessibility: the trigger is a `Button` named by the label, or by the
//! item's title, whose expanded state follows the content; its description
//! is a disabled reason, else the item's description. GPUI cannot name
//! another node, so the trigger does not name the content it controls.
//!
//! Reference: WAI-ARIA's disclosure pattern and Radix's Collapsible.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Corners, Div, ElementId, FocusHandle, IntoElement,
    MouseButton, ParentElement, Pixels, RenderOnce, SharedString, Stateful,
    StyleRefinement, Svg, Window, div, prelude::*, px, svg,
};

use crate::a11y::States;
use crate::click::from_keyboard;
use crate::components::item::{self, Item, Parts};
use crate::components::tooltip::Tooltip;
use crate::focus;
use crate::tokens::{Tokens, size};

type Toggle = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A collapsible section.
#[derive(IntoElement)]
pub struct Collapsible {
    id: ElementId,
    label: SharedString,
    open: bool,
    focus: FocusHandle,
    tokens: Tokens,
    item: Option<Item>,
    corners: Option<Corners<Pixels>>,
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
            .field("item", &self.item)
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
            item: None,
            corners: None,
            disabled: None,
            key_context: None,
            on_toggle: None,
            content: Vec::new(),
        }
    }

    /// Draws the header as `item`, such as one with a [`chevron`] for its
    /// media: the trigger covers its media, title, and description and is
    /// named by its title, and its actions sit beside the trigger.
    #[must_use]
    pub fn item(mut self, item: Item) -> Self {
        self.item = Some(item);
        self
    }

    /// The radii of the trigger's fill and focus ring, such as
    /// [`crate::components::card::row_corners`] for a row of a card. Beside
    /// an item's actions, the trigger's right corners stay square, since
    /// they meet no edge.
    #[must_use]
    pub fn corners(mut self, corners: Corners<Pixels>) -> Self {
        self.corners = Some(corners);
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

/// The open state the owner's activation command sets on a section whose
/// trigger holds `focus` and is `open` now: the other state while the
/// trigger has focus and the section is `enabled`, as the owner holds it
/// when the command arrives. `None` means the command does nothing.
#[must_use]
pub fn toggled(
    focus: &FocusHandle,
    open: bool,
    enabled: bool,
    window: &Window,
) -> Option<bool> {
    (enabled && focus.is_focused(window)).then_some(!open)
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

/// The trigger's corner radii: the owner's, with the right corners square
/// beside `actions`, else the control radius.
fn trigger_corners(
    corners: Option<Corners<Pixels>>,
    actions: bool,
) -> Corners<Pixels> {
    match corners {
        None => Corners::all(px(size::CONTROL_RADIUS)),
        Some(corners) if actions => Corners {
            top_right: Pixels::ZERO,
            bottom_right: Pixels::ZERO,
            ..corners
        },
        Some(corners) => corners,
    }
}

/// The trigger filled with the item's media and content, padded as the
/// item's row, with the item's actions and style; or, with no item, the
/// trigger drawing `label`.
fn lay_out(
    trigger: Stateful<Div>,
    parts: Option<Parts>,
    label: SharedString,
    tokens: Tokens,
) -> (Stateful<Div>, Option<Div>, StyleRefinement) {
    let Some(parts) = parts else {
        let trigger = trigger
            .px(px(8.0))
            .py(px(4.0))
            .text_color(tokens.fg)
            .child(label);
        return (trigger, None, StyleRefinement::default());
    };
    let actions = parts.actions.is_some();
    let trigger = trigger
        .flex()
        .pl(px(item::PADDING_X))
        .pr(px(if actions {
            item::GAP / 2.0
        } else {
            item::PADDING_X
        }))
        .py(px(item::PADDING_Y))
        .child(parts.lead);
    let actions = parts
        .actions
        .map(|actions| actions.pl(px(item::GAP / 2.0)).pr(px(item::PADDING_X)));
    (trigger, actions, parts.style)
}

impl RenderOnce for Collapsible {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let open = self.open;
        let focus = self.focus.clone().tab_stop(enabled);
        let reason = self.disabled.clone().filter(|reason| !reason.is_empty());
        let parts = self.item.map(Item::into_parts);
        let actions =
            parts.as_ref().is_some_and(|parts| parts.actions.is_some());
        let corners = trigger_corners(self.corners, actions);
        let (label, description) = match &parts {
            Some(parts) => (parts.title.clone(), parts.description.clone()),
            None => (self.label.clone(), None),
        };
        let trigger = div()
            .id(self.id)
            .role(gpui::Role::Button)
            .aria_label(label)
            .aria_expanded(open)
            .when_some(reason.clone().or(description), |trigger, text| {
                trigger.aria_description(text)
            })
            .when_some(reason, |trigger, reason| {
                trigger.tooltip(move |_, cx| {
                    Tooltip::view(reason.clone(), tokens, cx)
                })
            })
            .track_focus(&focus)
            .when_some(self.key_context, |trigger, context| {
                trigger.key_context(context.as_ref())
            })
            .flex_1()
            .min_w_0()
            .rounded_tl(corners.top_left)
            .rounded_tr(corners.top_right)
            .rounded_br(corners.bottom_right)
            .rounded_bl(corners.bottom_left)
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
            .when_some(
                self.on_toggle.filter(|_| enabled),
                |trigger, toggle| {
                    trigger.on_click(move |event, window, cx| {
                        if !from_keyboard(event) {
                            toggle(!open, window, cx);
                        }
                    })
                },
            );
        let (trigger, actions, style) =
            lay_out(trigger, parts, self.label, tokens);
        let trigger = focus::ring_inset(trigger, &focus, tokens, window, cx);
        let trigger = States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(trigger);
        let mut header =
            div().flex().items_center().child(trigger).children(actions);
        header.style().refine(&style);
        div()
            .flex()
            .flex_col()
            .child(header)
            .when(open, |section| section.children(self.content))
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use gpui::{
        AccessibleAction, App, FocusHandle, KeyUpEvent, Keystroke, Modifiers,
        TestAppContext, VisualTestContext, Window, div, prelude::*, px,
    };

    use super::{Collapsible, toggled};
    use crate::components::card::{Card, row_corners};
    use crate::components::item::Item;
    use crate::focus::{inset_ring_drawn, ring_drawn};
    use crate::testing::{
        Activate, KEYS, a11y_action, a11y_node, bind_keys, host, tokens,
    };

    struct Probe {
        focus: FocusHandle,
        before: FocusHandle,
        toggles: Rc<RefCell<Vec<bool>>>,
        /// Presses of the row's Reset action.
        resets: Rc<Cell<usize>>,
        /// Whether the section is enabled; a test may change it.
        enabled: Rc<Cell<bool>>,
    }

    /// A focusable stop, then a closed section in a card, with one line of
    /// content, whose header is an item with a Reset action, that toggles
    /// as an owner does: on a pointer click, and on Space routed to it
    /// through [`toggled`].
    fn section(
        cx: &mut TestAppContext,
        disabled: bool,
    ) -> (Probe, &mut VisualTestContext) {
        let focus = cx.update(|cx| cx.focus_handle());
        let before = cx.update(|cx| cx.focus_handle().tab_stop(true));
        let toggles = Rc::new(RefCell::new(Vec::new()));
        let resets = Rc::new(Cell::new(0));
        let enabled = Rc::new(Cell::new(!disabled));
        let open = Rc::new(Cell::new(false));
        let (drawn, drawn_before, seen, pressed, live) = (
            focus.clone(),
            before.clone(),
            toggles.clone(),
            resets.clone(),
            enabled.clone(),
        );
        let (_, cx) = host(cx, move |_, _| {
            let toggle = {
                let (seen, open) = (seen.clone(), open.clone());
                move |next: bool, window: &mut Window| {
                    seen.borrow_mut().push(next);
                    open.set(next);
                    window.refresh();
                }
            };
            let pressed = pressed.clone();
            let reset = div()
                .id("reset")
                .role(gpui::Role::Button)
                .aria_label("Reset")
                .size(px(20.0))
                .debug_selector(|| "reset".into())
                .on_click(move |_, _, _| pressed.set(pressed.get() + 1));
            let header = Item::new("copy-item", "Copy", tokens())
                .description("Copy the selection")
                .action(reset);
            let section =
                Collapsible::new("copy", "Copy", open.get(), &drawn, tokens())
                    .item(header)
                    .corners(row_corners(0, 1))
                    .key_context(KEYS)
                    .child(
                        div()
                            .id("shortcuts")
                            .role(gpui::Role::Label)
                            .aria_label("Shortcuts"),
                    );
            let section = if live.get() {
                let toggle = toggle.clone();
                section.on_toggle(move |next, window, _| toggle(next, window))
            } else {
                section.disabled(Some("The file is invalid".into()))
            };
            let (open, routed, live) =
                (open.clone(), drawn.clone(), live.clone());
            div()
                .child(
                    div()
                        .id("before")
                        .track_focus(&drawn_before)
                        .size(px(10.0)),
                )
                .child(
                    div()
                        .w(px(200.0))
                        .child(Card::new("card", tokens()).child(section)),
                )
                .on_action(move |_: &Activate, window, _: &mut App| {
                    if let Some(next) =
                        toggled(&routed, open.get(), live.get(), window)
                    {
                        toggle(next, window);
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
                resets,
                enabled,
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
            assert_eq!(trigger["description"], "Copy the selection");
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
    fn a_press_on_an_action_runs_it_and_does_not_toggle(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = section(cx, false);
        let reset = cx.debug_bounds("reset").expect("the action drew");
        cx.simulate_click(reset.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(probe.resets.get(), 1);
        assert!(probe.toggles.borrow().is_empty());
        assert!(!cx.update(|window, _| probe.focus.is_focused(window)));
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
    }

    #[gpui::test]
    fn a_focused_trigger_in_a_card_draws_its_ring_inside(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = section(cx, false);
        cx.simulate_keystrokes("a");
        cx.update(|window, cx| probe.focus.focus(window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(inset_ring_drawn(&probe.focus, window, cx));
            // The card clips an outer ring.
            assert!(!ring_drawn(&probe.focus, window, cx));
        });
    }

    #[gpui::test]
    fn a_section_disabled_under_focus_refuses_its_key(cx: &mut TestAppContext) {
        let (probe, cx) = section(cx, false);
        cx.update(|window, cx| probe.focus.focus(window, cx));
        probe.enabled.set(false);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.update(|window, _| {
            assert_eq!(a11y_node(window, "copy")["aria"]["disabled"], true);
        });
        cx.simulate_keystrokes("space");
        assert!(probe.toggles.borrow().is_empty());
        assert!(!shows_content(cx));
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
