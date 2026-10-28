//! Tabs: a list of sections, one of them shown, such as the Settings
//! sidebar, after shadcn/ui's and Radix's Tabs.
//!
//! Anatomy: a tab list laid out horizontally or vertically, whose selected
//! tab takes the selection fill, and the [`panel`] that shows the selected
//! section, which the owner places where its layout wants it. A tab may
//! carry a trailing element, such as a count.
//!
//! Keyboard: the list is one tab stop, through [`crate::roving`]: the
//! focused tab, else the selected one, else the first. The owner routes
//! its move commands through [`crate::roving::move_focus`], with
//! [`crate::roving::Move::along`] for the arrows of the list's axis (Up and
//! Down when vertical, Left and Right when horizontal), and Home and End,
//! and selects the tab focus reaches: activation is automatic, as Radix's
//! default is. Moves wrap; a move onto the selected tab reports nothing.
//! GPUI's implicit keyboard click is ignored, so only routed commands act
//! on keys.
//!
//! Pointer: a click on a tab focuses and selects it. A disabled list takes
//! no focus from a press, does not select, drops its tab stop, refuses its
//! routed commands (the owner passes its live enabled state to
//! [`crate::roving::move_focus`]), and shows its reason as a tooltip.
//!
//! Focus: the focused tab draws the focus ring after a key: around a
//! horizontal tab, and inside a vertical one, which fills the list's width
//! and would lose an outer ring to a sidebar that clips its overflow.
//!
//! Accessibility: a `TabList` named by its label, with its orientation and
//! size, of `Tab`s whose selected state follows the selection (macOS reads
//! a tab's `selected`), with their position in the list; the panel is
//! a `TabPanel` named by the selected tab. GPUI cannot name another node,
//! so the tab and its panel are not related in the tree. Each tab reports a
//! disabled list's disabled state, since AccessKit states are per node.
//!
//! Reference: WAI-ARIA's tabs pattern with automatic activation, and
//! Radix's Tabs.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Axis, Div, ElementId, FocusHandle, IntoElement,
    MouseButton, RenderOnce, SharedString, Stateful, Window, div, prelude::*,
    px,
};

use crate::a11y::States;
use crate::click::from_keyboard;
use crate::components::tooltip::Tooltip;
use crate::focus;
use crate::tokens::{Tokens, size};

type Select = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

/// One tab.
struct Tab {
    value: SharedString,
    label: SharedString,
    selected: bool,
    focus: FocusHandle,
    trailing: Option<AnyElement>,
}

/// A tab list.
#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    label: SharedString,
    axis: Axis,
    tokens: Tokens,
    items: Vec<Tab>,
    disabled: Option<SharedString>,
    key_context: Option<SharedString>,
    on_select: Option<Select>,
}

impl std::fmt::Debug for Tabs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tabs: Vec<_> = self.items.iter().map(|tab| &tab.value).collect();
        f.debug_struct("Tabs")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("axis", &self.axis)
            .field("tabs", &tabs)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

impl Tabs {
    /// A horizontal tab list named `label`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            axis: Axis::Horizontal,
            tokens,
            items: Vec::new(),
            disabled: None,
            key_context: None,
            on_select: None,
        }
    }

    /// Lays the tabs out top to bottom, as a sidebar, each as wide as the
    /// list.
    #[must_use]
    pub fn vertical(mut self) -> Self {
        self.axis = Axis::Vertical;
        self
    }

    /// The axis the tabs run along, for [`crate::roving::Move::along`].
    #[must_use]
    pub fn axis(&self) -> Axis {
        self.axis
    }

    /// A tab for `value`, reading `label`, `selected` or not, focused
    /// through `focus`.
    #[must_use]
    pub fn tab(
        mut self,
        value: impl Into<SharedString>,
        label: impl Into<SharedString>,
        selected: bool,
        focus: &FocusHandle,
    ) -> Self {
        self.items.push(Tab {
            value: value.into(),
            label: label.into(),
            selected,
            focus: focus.clone(),
            trailing: None,
        });
        self
    }

    /// An element after the label of the tab added last, such as a count.
    #[must_use]
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        let tab = self.items.last_mut();
        debug_assert!(tab.is_some(), "`trailing` follows a tab");
        if let Some(tab) = tab {
            tab.trailing = Some(element.into_any_element());
        }
        self
    }

    /// Disables the list; `reason`, when given, shows as its tooltip and
    /// accessible description.
    #[must_use]
    pub fn disabled(mut self, reason: Option<SharedString>) -> Self {
        self.disabled = Some(reason.unwrap_or_default());
        self
    }

    /// The key context every tab carries, such as the owner's control
    /// context that its move commands bind under.
    #[must_use]
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    /// Called with a tab's value when a pointer click selects it. A click
    /// on the selected tab reports nothing.
    #[must_use]
    pub fn on_select(
        mut self,
        select: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(select));
        self
    }
}

/// The panel showing the selected tab's section, named by that tab's
/// `label`. Add the section's content as children.
#[must_use]
pub fn panel(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(gpui::Role::TabPanel)
        .aria_label(label.into())
}

/// Where a tab sits and how its list draws it.
struct Shape {
    parent: Arc<ElementId>,
    index: usize,
    vertical: bool,
    enabled: bool,
    stop: bool,
    key_context: Option<SharedString>,
}

fn render_tab(
    tab: Tab,
    shape: Shape,
    select: Option<Select>,
    tokens: Tokens,
    window: &Window,
    cx: &mut App,
) -> Stateful<Div> {
    let Shape {
        parent,
        index,
        vertical,
        enabled,
        stop,
        key_context,
    } = shape;
    let focus = tab.focus.tab_stop(enabled && stop);
    let selected = tab.selected;
    let value = tab.value.clone();
    let element = div()
        .id(ElementId::NamedChild(parent, tab.value))
        .role(gpui::Role::Tab)
        .aria_label(tab.label.clone())
        .aria_selected(selected)
        // AccessKit positions are zero-based; the list carries the size.
        .aria_position_in_set(index)
        .track_focus(&focus)
        .when_some(key_context, |element, context| {
            element.key_context(context.as_ref())
        })
        .flex()
        .items_center()
        .gap(px(6.0))
        .when(vertical, Styled::w_full)
        .when(!vertical, Styled::flex_none)
        .h(size::ROW_HEIGHT)
        .px(px(8.0))
        .rounded(px(size::CONTROL_RADIUS))
        .text_size(size::TEXT_BODY)
        .text_color(tokens.fg)
        .when(selected, |element| element.bg(tokens.selection))
        .when(enabled && !selected, |element| {
            element
                .hover(|style| style.bg(tokens.hover))
                .active(|style| style.bg(tokens.pressed()))
        })
        .when(enabled && selected, |element| {
            element.active(|style| style.bg(tokens.selection_pressed()))
        })
        // A tab takes focus from a press before the list's guard sees it,
        // so a disabled tab refuses the press itself.
        .when(!enabled, |element| {
            element.on_mouse_down(MouseButton::Left, |_, window, _| {
                window.prevent_default();
            })
        })
        .when_some(select.filter(|_| !selected), |element, select| {
            element.on_click(move |event, window, cx| {
                if !from_keyboard(event) {
                    select(value.clone(), window, cx);
                }
            })
        })
        .child(div().flex_1().min_w_0().truncate().child(tab.label))
        .children(tab.trailing);
    let element = if vertical {
        focus::ring_inset(element, &focus, tokens, window, cx)
    } else {
        focus::ring(element, &focus, tokens, window, cx)
    };
    States {
        disabled: !enabled,
        ..States::default()
    }
    .apply(element)
}

impl RenderOnce for Tabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let vertical = self.axis == Axis::Vertical;
        let count = self.items.len();
        let parent = Arc::new(self.id.clone());
        debug_assert!(
            self.items.iter().filter(|tab| tab.selected).count() <= 1,
            "a tab list selects at most one tab"
        );
        let stop = crate::roving::stop(
            self.items
                .iter()
                .position(|tab| tab.focus.is_focused(window)),
            self.items.iter().position(|tab| tab.selected),
        );
        let tabs = self
            .items
            .into_iter()
            .enumerate()
            .map(|(index, tab)| {
                let shape = Shape {
                    parent: parent.clone(),
                    index,
                    vertical,
                    enabled,
                    stop: index == stop,
                    key_context: self.key_context.clone(),
                };
                let select = self.on_select.clone().filter(|_| enabled);
                render_tab(tab, shape, select, tokens, window, cx)
            })
            .collect::<Vec<_>>();
        let reason = self.disabled.filter(|reason| !reason.is_empty());
        let list = div()
            .id(self.id)
            .role(gpui::Role::TabList)
            .aria_label(self.label)
            .aria_orientation(if vertical {
                gpui::Orientation::Vertical
            } else {
                gpui::Orientation::Horizontal
            })
            .aria_size_of_set(count)
            .when_some(reason, |list, reason| {
                list.aria_description(reason.clone()).tooltip(move |_, cx| {
                    Tooltip::view(reason.clone(), tokens, cx)
                })
            })
            .flex()
            .gap(px(2.0))
            .when(vertical, Styled::flex_col)
            .when(!vertical, Styled::items_center)
            .when(!enabled, |list| list.opacity(0.5))
            .children(tabs);
        States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(list)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
