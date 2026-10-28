//! Toggle group: a row of connected items, either one choice among a few
//! (a segmented control, as `NSSegmentedControl` is) or several independent
//! toggles (chips, such as the link modifiers).
//!
//! Anatomy: items in one outlined row; a selected item takes the selection
//! fill.
//!
//! Keyboard, single: the group is one tab stop; the owner moves the
//! selection with its step commands (the arrow keys by default), which
//! selects and applies, as a radio group does. Multiple: one roving tab
//! stop, the focused item or else the first; the owner moves focus between
//! items with its step commands through [`roving_target`], wrapping, and
//! toggles the focused item with its activation command. GPUI's implicit
//! keyboard click is ignored, so only routed commands act on keys.
//!
//! Pointer: a click selects an item (single) or toggles it (multiple). A
//! disabled group takes no focus from a press and does not act.
//!
//! Accessibility, single: a `RadioGroup` of `RadioButton`s whose toggled
//! state marks the selection, with their position in the set. Multiple: a
//! `Group` of `Button`s whose toggled state follows each item.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Div, ElementId, FocusHandle, IntoElement, MouseButton, RenderOnce,
    SharedString, Stateful, Toggled, Window, div, prelude::*, px,
};

use crate::a11y::States;
use crate::click::from_keyboard;
use crate::focus;
use crate::tokens::{Tokens, size};

/// Whether a group picks one item or toggles several.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// One selected item, as a segmented control or radio group.
    Single,
    /// Independent toggles, as chips.
    Multiple,
}

/// One item.
#[derive(Clone, Debug)]
struct Item {
    value: SharedString,
    label: SharedString,
    on: bool,
    /// The item's own handle in a multiple group.
    focus: Option<FocusHandle>,
}

type Select = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

/// A toggle group.
#[derive(IntoElement)]
pub struct ToggleGroup {
    id: ElementId,
    label: SharedString,
    mode: Mode,
    tokens: Tokens,
    items: Vec<Item>,
    focus: Option<FocusHandle>,
    disabled: Option<SharedString>,
    key_context: Option<SharedString>,
    on_select: Option<Select>,
}

impl std::fmt::Debug for ToggleGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToggleGroup")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("mode", &self.mode)
            .field("items", &self.items)
            .finish_non_exhaustive()
    }
}

impl ToggleGroup {
    /// A single-choice group named `label`, one tab stop through `focus`.
    #[must_use]
    pub fn single(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        focus: &FocusHandle,
        tokens: Tokens,
    ) -> Self {
        Self::new(
            id.into(),
            label.into(),
            Mode::Single,
            Some(focus.clone()),
            tokens,
        )
    }

    /// A group of independent toggles named `label`; each item brings its
    /// own focus handle through [`Self::toggle`].
    #[must_use]
    pub fn multiple(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        tokens: Tokens,
    ) -> Self {
        Self::new(id.into(), label.into(), Mode::Multiple, None, tokens)
    }

    fn new(
        id: ElementId,
        label: SharedString,
        mode: Mode,
        focus: Option<FocusHandle>,
        tokens: Tokens,
    ) -> Self {
        Self {
            id,
            label,
            mode,
            tokens,
            items: Vec::new(),
            focus,
            disabled: None,
            key_context: None,
            on_select: None,
        }
    }

    /// A single group's item `value`, reading `label`, selected when `on`.
    #[must_use]
    pub fn option(
        mut self,
        value: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on: bool,
    ) -> Self {
        self.items.push(Item {
            value: value.into(),
            label: label.into(),
            on,
            focus: None,
        });
        self
    }

    /// A multiple group's item `value`, reading `label`, on when `on`, and
    /// focused through `focus`.
    #[must_use]
    pub fn toggle(
        mut self,
        value: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on: bool,
        focus: &FocusHandle,
    ) -> Self {
        self.items.push(Item {
            value: value.into(),
            label: label.into(),
            on,
            focus: Some(focus.clone()),
        });
        self
    }

    /// Disables the group; `reason` becomes its accessible description.
    #[must_use]
    pub fn disabled(mut self, reason: Option<SharedString>) -> Self {
        self.disabled = Some(reason.unwrap_or_default());
        self
    }

    /// The key context the group and its items carry, such as the owner's
    /// control context that its step and activation commands bind under.
    #[must_use]
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    /// Called with an item's value for a pointer click: the item to select
    /// in a single group, or to toggle in a multiple one.
    #[must_use]
    pub fn on_select(
        mut self,
        select: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(select));
        self
    }
}

/// The handle a multiple group's focus moves to from the focused item,
/// `forward` or back, wrapping; the first item when none has focus.
#[must_use]
pub fn roving_target(
    items: &[FocusHandle],
    forward: bool,
    window: &Window,
) -> Option<FocusHandle> {
    let count = items.len();
    let Some(current) = items.iter().position(|item| item.is_focused(window))
    else {
        return items.first().cloned();
    };
    let next = if forward {
        (current + 1) % count
    } else {
        (current + count - 1) % count
    };
    items.get(next).cloned()
}

/// Where an item sits and how its group draws it.
struct Shape {
    parent: Arc<ElementId>,
    single: bool,
    enabled: bool,
    index: usize,
    count: usize,
    /// Whether a multiple group's item is its roving tab stop.
    stop: bool,
    key_context: Option<SharedString>,
}

fn render_item(
    item: Item,
    shape: Shape,
    select: Option<Select>,
    tokens: Tokens,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let Shape {
        parent,
        single,
        enabled,
        index,
        count,
        stop,
        key_context,
    } = shape;
    let id = ElementId::NamedChild(parent, item.value.clone());
    let focus = item.focus.map(|focus| focus.tab_stop(enabled && stop));
    let value = item.value.clone();
    let element = div()
        .id(id)
        .role(if single {
            gpui::Role::RadioButton
        } else {
            gpui::Role::Button
        })
        .aria_label(item.label.clone())
        .aria_toggled(if item.on {
            Toggled::True
        } else {
            Toggled::False
        })
        .aria_position_in_set(index + 1)
        .aria_size_of_set(count)
        .when_some(focus.as_ref(), InteractiveElement::track_focus)
        .when_some(key_context, |element, context| {
            element.key_context(context.as_ref())
        })
        .px(px(10.0))
        .h(px(22.0))
        .flex()
        .items_center()
        .text_size(size::TEXT_CONTROL)
        .text_color(tokens.fg)
        .when(!single, |element| {
            element
                .rounded(px(5.0))
                .border_1()
                .border_color(if item.on {
                    tokens.accent
                } else {
                    tokens.line_strong
                })
        })
        .when(item.on, |element| element.bg(tokens.selection))
        .when(enabled && !item.on, |element| {
            element.hover(|style| style.bg(tokens.hover))
        })
        .when(enabled, |element| {
            element.active(|style| style.bg(tokens.selection_pressed()))
        })
        // A focusable item takes focus from a press before the group's own
        // guard sees it, so a disabled item refuses the press itself.
        .when(!enabled && focus.is_some(), |element| {
            element.on_mouse_down(MouseButton::Left, |_, window, _| {
                window.prevent_default();
            })
        })
        .when_some(select, |element, select| {
            element.on_click(move |event, window, cx| {
                if !from_keyboard(event) {
                    select(value.clone(), window, cx);
                }
            })
        })
        .child(item.label);
    match &focus {
        Some(focus) => focus::ring(element, focus, tokens, window, cx),
        None => element,
    }
}

impl RenderOnce for ToggleGroup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let single = self.mode == Mode::Single;
        let count = self.items.len();
        let parent = Arc::new(self.id.clone());
        // A multiple group's tab stop is its focused item, else its first.
        let roving = self
            .items
            .iter()
            .position(|item| {
                item.focus.as_ref().is_some_and(|f| f.is_focused(window))
            })
            .unwrap_or(0);
        let items = self
            .items
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let shape = Shape {
                    parent: parent.clone(),
                    single,
                    enabled,
                    index,
                    count,
                    stop: index == roving,
                    key_context: self.key_context.clone().filter(|_| !single),
                };
                let select = self.on_select.clone().filter(|_| enabled);
                render_item(item, shape, select, tokens, window, cx)
            })
            .collect::<Vec<_>>();
        let reason = self.disabled.clone().filter(|reason| !reason.is_empty());
        let group_focus = self.focus.map(|focus| focus.tab_stop(enabled));
        let group = div()
            .id(self.id)
            .role(if single {
                gpui::Role::RadioGroup
            } else {
                gpui::Role::Group
            })
            .aria_label(self.label)
            .when_some(reason, StatefulInteractiveElement::aria_description)
            .when_some(group_focus.as_ref(), |group, focus| {
                group.track_focus(focus)
            })
            .when_some(self.key_context.filter(|_| single), |group, context| {
                group.key_context(context.as_ref())
            })
            .flex()
            .flex_none()
            .when(single, |group| {
                group
                    .rounded(px(size::CONTROL_RADIUS))
                    .border_1()
                    .border_color(tokens.line_strong)
                    .overflow_hidden()
            })
            .when(!single, |group| group.gap(px(4.0)))
            .when(!enabled, |group| {
                group.opacity(0.5).on_mouse_down(
                    MouseButton::Left,
                    |_, window, _| {
                        window.prevent_default();
                    },
                )
            })
            .children(items);
        let group = match &group_focus {
            Some(focus) => focus::ring(group, focus, tokens, window, cx),
            None => group,
        };
        States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(group)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{
        AccessibleAction, FocusHandle, KeyUpEvent, Keystroke, SharedString,
        TestAppContext, VisualTestContext, Window, div, prelude::*,
    };

    use super::{ToggleGroup, roving_target};
    use crate::components::button::Button;
    use crate::testing::{a11y_labelled, a11y_node, a11y_send, host, tokens};

    struct Probe {
        focus: FocusHandle,
        picks: Rc<RefCell<Vec<SharedString>>>,
    }

    fn single(
        cx: &mut TestAppContext,
        disabled: bool,
    ) -> (Probe, &mut VisualTestContext) {
        let focus = cx.update(|cx| cx.focus_handle());
        let picks = Rc::new(RefCell::new(Vec::new()));
        let (drawn, seen) = (focus.clone(), picks.clone());
        let (_, cx) = host(cx, move |_, _| {
            let seen = seen.clone();
            let group = ToggleGroup::single(
                "tabs",
                "Tab bar position",
                &drawn,
                tokens(),
            )
            .option("top", "Top", true)
            .option("bottom", "Bottom", false)
            .on_select(move |value, _, _| seen.borrow_mut().push(value));
            let group = if disabled {
                group.disabled(Some("The file is invalid".into()))
            } else {
                group
            };
            group.into_any_element()
        });
        (Probe { focus, picks }, cx)
    }

    fn click(cx: &mut VisualTestContext, label: &'static str) {
        a11y_send(
            cx,
            |window| a11y_labelled(window, label),
            AccessibleAction::Click,
        );
    }

    #[gpui::test]
    fn a_single_group_reports_a_radio_group(cx: &mut TestAppContext) {
        let (_, cx) = single(cx, false);
        cx.update(|window, _| {
            let group = &a11y_node(window, "tabs")["aria"];
            assert_eq!(group["role"], "RadioGroup");
            assert_eq!(group["label"], "Tab bar position");
            let top = &a11y_labelled(window, "Top")["aria"];
            assert_eq!(top["role"], "RadioButton");
            assert_eq!(top["toggled"], "True");
            assert_eq!(top["position_in_set"], 1);
            assert_eq!(top["size_of_set"], 2);
            let bottom = &a11y_labelled(window, "Bottom")["aria"];
            assert_eq!(bottom["toggled"], "False");
            assert_eq!(bottom["position_in_set"], 2);
        });
    }

    #[gpui::test]
    fn a_pointer_click_selects_and_focuses_the_group(cx: &mut TestAppContext) {
        let (probe, cx) = single(cx, false);
        click(cx, "Bottom");
        assert_eq!(*probe.picks.borrow(), ["bottom"]);
        assert!(cx.update(|window, _| probe.focus.is_focused(window)));
    }

    #[gpui::test]
    fn a_disabled_group_neither_selects_nor_takes_focus(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = single(cx, true);
        click(cx, "Bottom");
        assert!(probe.picks.borrow().is_empty());
        assert!(!cx.update(|window, _| probe.focus.is_focused(window)));
        cx.update(|window, _| {
            let group = &a11y_node(window, "tabs")["aria"];
            assert_eq!(group["disabled"], true);
            assert_eq!(group["description"], "The file is invalid");
        });
    }

    struct Chips {
        items: Vec<FocusHandle>,
        after: FocusHandle,
        picks: Rc<RefCell<Vec<SharedString>>>,
    }

    /// A multiple group of three chips with Alt on, then a focusable button.
    fn multiple(
        cx: &mut TestAppContext,
        disabled: bool,
    ) -> (Chips, &mut VisualTestContext) {
        let items: Vec<FocusHandle> =
            cx.update(|cx| (0..3).map(|_| cx.focus_handle()).collect());
        let after = cx.update(|cx| cx.focus_handle());
        let picks = Rc::new(RefCell::new(Vec::new()));
        let (drawn, next, seen) = (items.clone(), after.clone(), picks.clone());
        let (_, cx) = host(cx, move |_, _| {
            let seen = seen.clone();
            let group = ["Cmd", "Alt", "Ctrl"]
                .iter()
                .zip(&drawn)
                .fold(
                    ToggleGroup::multiple("mods", "Link modifiers", tokens()),
                    |group, (label, focus)| {
                        group.toggle(*label, *label, *label == "Alt", focus)
                    },
                )
                .on_select(move |value, _, _| seen.borrow_mut().push(value));
            let group = if disabled {
                group.disabled(None)
            } else {
                group
            };
            div()
                .child(group)
                .child(Button::new("after", "After", tokens()).focus(&next))
                .into_any_element()
        });
        (
            Chips {
                items,
                after,
                picks,
            },
            cx,
        )
    }

    fn focused(cx: &mut VisualTestContext, handle: &FocusHandle) -> bool {
        cx.update(|window, _| handle.is_focused(window))
    }

    #[gpui::test]
    fn a_multiple_group_reports_toggle_buttons(cx: &mut TestAppContext) {
        let (_, cx) = multiple(cx, false);
        cx.update(|window, _| {
            assert_eq!(a11y_node(window, "mods")["aria"]["role"], "Group");
            let alt = &a11y_labelled(window, "Alt")["aria"];
            assert_eq!(alt["role"], "Button");
            assert_eq!(alt["toggled"], "True");
            assert_eq!(
                a11y_labelled(window, "Cmd")["aria"]["toggled"],
                "False"
            );
        });
    }

    #[gpui::test]
    fn a_pointer_click_toggles_an_item_and_an_implicit_key_click_does_not(
        cx: &mut TestAppContext,
    ) {
        let (chips, cx) = multiple(cx, false);
        click(cx, "Ctrl");
        assert_eq!(*chips.picks.borrow(), ["Ctrl"]);
        assert!(focused(cx, &chips.items[2]));
        // GPUI clicks the focused item on Space's key-up; nothing binds
        // Space here, so only that implicit click could toggle.
        cx.simulate_keystrokes("space");
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("space").expect("a valid keystroke"),
        });
        cx.run_until_parked();
        assert_eq!(*chips.picks.borrow(), ["Ctrl"]);
    }

    #[gpui::test]
    fn a_disabled_multiple_group_neither_toggles_nor_takes_focus(
        cx: &mut TestAppContext,
    ) {
        let (chips, cx) = multiple(cx, true);
        click(cx, "Ctrl");
        assert!(chips.picks.borrow().is_empty());
        assert!(!focused(cx, &chips.items[2]));
    }

    #[gpui::test]
    fn a_multiple_group_is_one_roving_tab_stop(cx: &mut TestAppContext) {
        let (chips, cx) = multiple(cx, false);
        let Chips { items, after, .. } = chips;
        // With no item focused, the first is the group's only stop.
        cx.update(Window::focus_next);
        cx.run_until_parked();
        assert!(focused(cx, &items[0]));
        cx.update(Window::focus_next);
        cx.run_until_parked();
        assert!(focused(cx, &after));
        // Focus moved inside the group makes that item the only stop, and
        // the stop returns to the first item once focus leaves.
        cx.update(|window, cx| items[1].focus(window, cx));
        cx.run_until_parked();
        cx.update(Window::focus_next);
        cx.run_until_parked();
        assert!(focused(cx, &after));
        cx.update(Window::focus_prev);
        cx.run_until_parked();
        assert!(focused(cx, &items[0]));
        // Stepping wraps at both ends.
        cx.update(|window, cx| items[2].focus(window, cx));
        cx.update(|window, _| {
            let next = roving_target(&items, true, window);
            assert_eq!(next.as_ref(), Some(&items[0]));
        });
        cx.update(|window, cx| items[0].focus(window, cx));
        cx.update(|window, _| {
            let back = roving_target(&items, false, window);
            assert_eq!(back.as_ref(), Some(&items[2]));
        });
    }
}
