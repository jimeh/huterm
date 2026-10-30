//! Input group: an [`Input`] with leading or trailing addons, after
//! shadcn/ui's Input Group: a search icon, a unit, a clear button, or a
//! number field's steppers.
//!
//! Anatomy: one control-height outlined box holding the leading addons, the
//! field, then the trailing addons. Text and icon addons draw as secondary
//! text; button addons draw as small buttons. The field must use
//! [`Chrome::Grouped`], so the group draws the only border and ring.
//!
//! Keyboard: the field keeps every key. An addon button the owner gives no
//! focus handle is not a tab stop, as in-field buttons in React Aria and
//! the native toolkits are not.
//!
//! Pointer: a press on a text or icon addon, or on a button that cannot
//! take focus, focuses the field, as a press on a label does; the button
//! still acts on its click.
//!
//! Focus: the group shows the focus ring while its field has keyboard
//! focus.
//!
//! States: the group follows its field. A disabled field dims the whole
//! group and disables every addon button, and a press anywhere in it
//! takes no focus, keeping the reason an owner gave a button it disabled
//! itself; an invalid field draws the group's border in the danger color. A
//! read-only field leaves its addons to the owner, which disables the
//! buttons that would edit it.
//!
//! Accessibility: the field reports itself; text and icon addons are
//! presentation, so a unit belongs in the field's label or description,
//! and button addons report their own names.
//!
//! Reference: shadcn/ui's Input Group.

use gpui::{
    AnyElement, App, ElementId, Entity, FocusHandle, Focusable as _,
    IntoElement, MouseButton, Pixels, RenderOnce, SharedString, Window, div,
    prelude::*, px,
};

use crate::components::button::Button;
use crate::components::input::{Chrome, Input};
use crate::focus;
use crate::tokens::{Tokens, size};

/// One addon beside the field.
#[expect(
    clippy::large_enum_variant,
    reason = "a group holds a few addons, built once per frame"
)]
pub enum Addon {
    /// A word of text, such as a unit.
    Text(SharedString),
    /// An icon, such as a search glass, at the text size.
    Icon(AnyElement),
    /// A button, drawn small, such as a clear button or a stepper.
    Button(Button),
}

impl std::fmt::Debug for Addon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text(text) => f.debug_tuple("Text").field(text).finish(),
            Self::Icon(_) => f.write_str("Icon"),
            Self::Button(button) => {
                f.debug_tuple("Button").field(button).finish()
            }
        }
    }
}

/// The gap around a button addon: to the group's border on its outer side,
/// above, and below, and to the next addon.
const ADDON_INSET: Pixels = px(2.0);

/// Which side of the field an addon sits on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Leading,
    Trailing,
}

/// A field with addons.
#[derive(IntoElement)]
pub struct InputGroup {
    id: ElementId,
    input: Entity<Input>,
    tokens: Tokens,
    leading: Vec<Addon>,
    trailing: Vec<Addon>,
}

impl std::fmt::Debug for InputGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputGroup")
            .field("id", &self.id)
            .field("leading", &self.leading)
            .field("trailing", &self.trailing)
            .finish_non_exhaustive()
    }
}

impl InputGroup {
    /// A group around `input`, which must use [`Chrome::Grouped`].
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        input: &Entity<Input>,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            input: input.clone(),
            tokens,
            leading: Vec::new(),
            trailing: Vec::new(),
        }
    }

    /// Adds `addon` before the field, after any added earlier.
    #[must_use]
    pub fn leading(mut self, addon: Addon) -> Self {
        self.leading.push(addon);
        self
    }

    /// Adds `addon` after the field, after any added earlier.
    #[must_use]
    pub fn trailing(mut self, addon: Addon) -> Self {
        self.trailing.push(addon);
        self
    }
}

/// Draws `addon` on `side`; a press on it focuses `focus` unless a
/// control inside already took focus, or the group is disabled.
fn addon(
    addon: Addon,
    side: Side,
    focus: &FocusHandle,
    enabled: bool,
    tokens: Tokens,
    rem: Pixels,
) -> AnyElement {
    let focus = focus.clone();
    let slot = div()
        .flex_none()
        .flex()
        .items_center()
        .h_full()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if !enabled {
                window.prevent_default();
            } else if !window.default_prevented() {
                // Keep a focusable ancestor from taking focus instead.
                focus.focus(window, cx);
                window.prevent_default();
            }
        });
    match addon {
        Addon::Text(text) => slot
            .map(|slot| inset(slot, side, px(8.0)))
            .text_size(size::TEXT_CONTROL)
            .text_color(tokens.muted)
            .child(text)
            .into_any_element(),
        Addon::Icon(icon) => slot
            .map(|slot| inset(slot, side, px(8.0)))
            .text_color(tokens.muted)
            .child(icon)
            .into_any_element(),
        Addon::Button(button) => {
            // Square, with the same gap to the group's border on its outer
            // side as above and below, and that gap again to its neighbor.
            let extent = size::CONTROL_HEIGHT.to_pixels(rem)
                - px(2.0)
                - ADDON_INSET * 2.0;
            let square = button.is_icon_only();
            let button = button
                .small()
                .h(extent)
                .when(square, |button| button.w(extent));
            // A button its owner disabled keeps its own reason.
            let button = if enabled || button.is_disabled() {
                button
            } else {
                button.disabled(None)
            };
            slot.map(|slot| inset(slot, side, ADDON_INSET))
                .child(button)
                .into_any_element()
        }
    }
}

/// `slot` padded on its outer side, where it meets the group's border.
fn inset(slot: gpui::Div, side: Side, amount: gpui::Pixels) -> gpui::Div {
    match side {
        Side::Leading => slot.pl(amount),
        Side::Trailing => slot.pr(amount),
    }
}

impl RenderOnce for InputGroup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let (focus, enabled, invalid) = {
            let input = self.input.read(cx);
            debug_assert_eq!(
                input.chrome(),
                Chrome::Grouped,
                "a grouped field draws no frame of its own"
            );
            (
                input.focus_handle(cx),
                !input.is_disabled(),
                input.is_invalid(),
            )
        };
        let rem = window.rem_size();
        let leading: Vec<AnyElement> = self
            .leading
            .into_iter()
            .map(|item| {
                addon(item, Side::Leading, &focus, enabled, tokens, rem)
            })
            .collect();
        let trailing: Vec<AnyElement> = self
            .trailing
            .into_iter()
            .map(|item| {
                addon(item, Side::Trailing, &focus, enabled, tokens, rem)
            })
            .collect();
        let group = div()
            .id(self.id)
            .flex()
            .items_center()
            .w_full()
            .h(size::CONTROL_HEIGHT)
            .rounded(px(size::CONTROL_RADIUS))
            .border_1()
            .border_color(if invalid {
                tokens.danger
            } else {
                tokens.line_strong
            })
            .bg(tokens.bg)
            .when(!enabled, |group| group.opacity(0.5))
            .children(leading)
            .child(self.input)
            .children(trailing);
        focus::ring(group, &focus, tokens, window, cx)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
