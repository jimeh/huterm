//! Radio group: one choice among options that each need more room than a
//! segment, after shadcn/ui's Radio Group, such as Settings' theme cards.
//!
//! Anatomy: by default, a column of rows, each a round indicator, filled
//! with a dot while checked, and its label. The card variant lays out
//! fixed-width cards that wrap: each shows the owner's content, such as a
//! theme's color preview, over its label, and a checked card draws its
//! border in the ring color.
//!
//! Keyboard: the group is one tab stop, through [`crate::roving`]: the
//! focused radio, else the checked one, else the first. The owner routes
//! every arrow through [`crate::roving::move_focus`] with
//! [`crate::roving::Move::along`] and no axis (Up and Left back, Down and
//! Right on, wrapping), and Home and End, and checks the radio focus
//! reaches, as WAI-ARIA's radio group does. Its activation command (Space
//! by default) checks the focused radio. GPUI's implicit keyboard click is
//! ignored, so only routed commands act on keys.
//!
//! Pointer: a click on a radio focuses and checks it. A disabled group
//! takes no focus from a press, does not check, drops its tab stop, dims,
//! and shows its reason as a tooltip.
//!
//! Focus: the focused radio, row or card, draws the focus ring after a key.
//!
//! Accessibility: a `RadioGroup` named by its label, with its size, of
//! `RadioButton`s whose toggled state marks the checked one (AccessKit's
//! macOS and Linux adapters read radio state from `toggled`), with their
//! position in the set. Each radio reports a disabled group's disabled
//! state, since AccessKit states are per node.
//!
//! Reference: WAI-ARIA's radio group pattern and Radix's Radio Group.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Div, ElementId, FocusHandle, IntoElement, MouseButton,
    Pixels, RenderOnce, SharedString, Stateful, Toggled, Window, div,
    prelude::*, px,
};

use crate::a11y::States;
use crate::click::from_keyboard;
use crate::components::tooltip::Tooltip;
use crate::focus;
use crate::tokens::{Tokens, size};

type Select = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

/// A card's corner radius.
const CARD_RADIUS: f32 = 7.0;
/// The default variant's indicator diameter.
const INDICATOR: f32 = 16.0;

/// One option.
struct Radio {
    value: SharedString,
    label: SharedString,
    checked: bool,
    focus: FocusHandle,
    content: Option<AnyElement>,
}

/// A radio group.
#[derive(IntoElement)]
pub struct RadioGroup {
    id: ElementId,
    label: SharedString,
    tokens: Tokens,
    radios: Vec<Radio>,
    /// The card variant's card width, or `None` for rows.
    card: Option<Pixels>,
    disabled: Option<SharedString>,
    key_context: Option<SharedString>,
    on_select: Option<Select>,
}

impl std::fmt::Debug for RadioGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let radios: Vec<_> =
            self.radios.iter().map(|radio| &radio.value).collect();
        f.debug_struct("RadioGroup")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("radios", &radios)
            .field("card", &self.card)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

impl RadioGroup {
    /// A group of rows named `label`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            tokens,
            radios: Vec::new(),
            card: None,
            disabled: None,
            key_context: None,
            on_select: None,
        }
    }

    /// Draws the options as cards `width` wide that wrap.
    #[must_use]
    pub fn card(mut self, width: Pixels) -> Self {
        self.card = Some(width);
        self
    }

    /// An option for `value`, reading `label`, `checked` or not, focused
    /// through `focus`.
    #[must_use]
    pub fn option(
        mut self,
        value: impl Into<SharedString>,
        label: impl Into<SharedString>,
        checked: bool,
        focus: &FocusHandle,
    ) -> Self {
        self.radios.push(Radio {
            value: value.into(),
            label: label.into(),
            checked,
            focus: focus.clone(),
            content: None,
        });
        self
    }

    /// What the option added last shows besides its label: above it on a
    /// card, such as a preview, and below it on a row.
    #[must_use]
    pub fn content(mut self, element: impl IntoElement) -> Self {
        let radio = self.radios.last_mut();
        debug_assert!(radio.is_some(), "`content` follows an option");
        if let Some(radio) = radio {
            radio.content = Some(element.into_any_element());
        }
        self
    }

    /// Disables the group; `reason`, when given, shows as its tooltip and
    /// accessible description.
    #[must_use]
    pub fn disabled(mut self, reason: Option<SharedString>) -> Self {
        self.disabled = Some(reason.unwrap_or_default());
        self
    }

    /// The key context every radio carries, such as the owner's control
    /// context that its move and activation commands bind under.
    #[must_use]
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    /// Called with an option's value when a pointer click checks it. A
    /// click on the checked option reports nothing.
    #[must_use]
    pub fn on_select(
        mut self,
        select: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(select));
        self
    }
}

/// Where a radio sits and how its group draws it.
struct Shape {
    parent: Arc<ElementId>,
    index: usize,
    card: Option<Pixels>,
    enabled: bool,
    stop: bool,
    key_context: Option<SharedString>,
}

/// A row's round indicator, with a dot while checked. The indicator alone
/// shows the state, so it draws in colors that reach 3:1.
fn indicator(checked: bool, tokens: Tokens) -> Div {
    div()
        .flex_none()
        .size(px(INDICATOR))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(if checked {
            tokens.ring()
        } else {
            tokens.state_line()
        })
        .when(checked, |ring| {
            ring.child(div().size(px(8.0)).rounded_full().bg(tokens.ring()))
        })
}

fn render_radio(
    radio: Radio,
    shape: Shape,
    select: Option<Select>,
    tokens: Tokens,
    window: &Window,
    cx: &mut App,
) -> Stateful<Div> {
    let Shape {
        parent,
        index,
        card,
        enabled,
        stop,
        key_context,
    } = shape;
    let focus = radio.focus.tab_stop(enabled && stop);
    let checked = radio.checked;
    let value = radio.value.clone();
    let label = div()
        .min_w_0()
        .truncate()
        .text_color(tokens.fg)
        .child(radio.label.clone());
    let element = div()
        .id(ElementId::NamedChild(parent, radio.value))
        .role(gpui::Role::RadioButton)
        .aria_label(radio.label)
        .aria_toggled(if checked {
            Toggled::True
        } else {
            Toggled::False
        })
        // AccessKit positions are zero-based; the group carries the size.
        .aria_position_in_set(index)
        .track_focus(&focus)
        .when_some(key_context, |element, context| {
            element.key_context(context.as_ref())
        })
        // A radio takes focus from a press before the group's guard sees
        // it, so a disabled radio refuses the press itself.
        .when(!enabled, |element| {
            element.on_mouse_down(MouseButton::Left, |_, window, _| {
                window.prevent_default();
            })
        })
        .when_some(select.filter(|_| !checked), |element, select| {
            element.on_click(move |event, window, cx| {
                if !from_keyboard(event) {
                    select(value.clone(), window, cx);
                }
            })
        });
    let element = match card {
        Some(width) => element
            .w(width)
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(4.0))
            .rounded(px(CARD_RADIUS))
            .border_1()
            // A checked card's border alone shows its state, so it keeps 3:1.
            .border_color(if checked { tokens.ring() } else { tokens.line })
            .when(enabled && !checked, |card| {
                card.hover(|style| style.border_color(tokens.line_strong))
            })
            .children(radio.content)
            .child(label.px(px(2.0)).text_size(size::TEXT_SMALL)),
        None => element
            .flex()
            .items_start()
            .gap(px(8.0))
            .py(px(4.0))
            .px(px(6.0))
            .rounded(px(size::CONTROL_RADIUS))
            .text_size(size::TEXT_BODY)
            .when(enabled, |row| {
                row.hover(|style| style.bg(tokens.hover))
                    .active(|style| style.bg(tokens.pressed()))
            })
            .child(indicator(checked, tokens).mt(px(1.0)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(2.0))
                    .child(label)
                    .children(radio.content),
            ),
    };
    let element = focus::ring(element, &focus, tokens, window, cx);
    States {
        disabled: !enabled,
        ..States::default()
    }
    .apply(element)
}

impl RenderOnce for RadioGroup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let count = self.radios.len();
        let parent = Arc::new(self.id.clone());
        debug_assert!(
            self.radios.iter().filter(|radio| radio.checked).count() <= 1,
            "a radio group checks at most one option"
        );
        let stop = crate::roving::stop(
            self.radios
                .iter()
                .position(|radio| radio.focus.is_focused(window)),
            self.radios.iter().position(|radio| radio.checked),
        );
        let card = self.card;
        let radios = self
            .radios
            .into_iter()
            .enumerate()
            .map(|(index, radio)| {
                let shape = Shape {
                    parent: parent.clone(),
                    index,
                    card,
                    enabled,
                    stop: index == stop,
                    key_context: self.key_context.clone(),
                };
                let select = self.on_select.clone().filter(|_| enabled);
                render_radio(radio, shape, select, tokens, window, cx)
            })
            .collect::<Vec<_>>();
        let reason = self.disabled.filter(|reason| !reason.is_empty());
        let group =
            div()
                .id(self.id)
                .role(gpui::Role::RadioGroup)
                .aria_label(self.label)
                .aria_size_of_set(count)
                .when_some(reason, |group, reason| {
                    group.aria_description(reason.clone()).tooltip(
                        move |_, cx| Tooltip::view(reason.clone(), tokens, cx),
                    )
                })
                .flex()
                .when(card.is_some(), |group| group.flex_wrap().gap(px(8.0)))
                .when(card.is_none(), |group| group.flex_col().gap(px(2.0)))
                .when(!enabled, |group| group.opacity(0.5))
                .children(radios);
        States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(group)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
