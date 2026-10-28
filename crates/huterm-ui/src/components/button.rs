//! Button: a labelled control that runs an action, such as Reset, Open
//! config.toml, or a dialog's Cancel and Delete.
//!
//! Anatomy: an optional icon and a label, optionally followed by a key cap
//! for its shortcut, in a control-height pill, or a shorter one for a
//! button inside another control, such as an input group's. An icon-only
//! button, such as the title row's, is a square that shows its label as a
//! tooltip and draws no key cap. [`Variant`]s follow shadcn/ui's: outlined
//! by default, a quiet secondary fill, a borderless ghost, an accent fill
//! for the primary action, a danger fill for destructive actions, and a
//! link.
//!
//! Keyboard: a button the owner gives a focus handle takes focus; the owner
//! runs its action on its activation command (Space and Return by default).
//! GPUI's implicit keyboard click is ignored, so only that routed command
//! acts on keys.
//!
//! Pointer: a click runs the action; hover and pressed states show. A
//! disabled button takes no focus from a press and does not act, and shows
//! its reason as a tooltip.
//!
//! Accessibility: a `Button` (or `Link`) named by its label, reporting its
//! shortcut, its tooltip or disabled reason as its description, and its
//! disabled state.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, ElementId, FocusHandle, Hsla, IntoElement,
    MouseButton, RenderOnce, SharedString, Stateful, Window, div, prelude::*,
    px,
};

use crate::a11y::States;
use crate::click::from_keyboard;
use crate::components::kbd::{self, Glyph};
use crate::components::tooltip::Tooltip;
use crate::focus;
use crate::tokens::{Tokens, size};

/// A button's look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    /// An outlined control.
    #[default]
    Outline,
    /// A quiet chip fill.
    Secondary,
    /// No border or fill until hovered.
    Ghost,
    /// The accent fill, for a view's primary action.
    Primary,
    /// The danger fill, for destructive actions.
    Destructive,
    /// Accent text without a pill.
    Link,
}

type Action = Rc<dyn Fn(&mut Window, &mut App)>;

/// A button.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: Variant,
    tokens: Tokens,
    focus: Option<FocusHandle>,
    disabled: Option<SharedString>,
    tooltip: Option<SharedString>,
    shortcut: Vec<Glyph>,
    icon: Option<AnyElement>,
    icon_only: bool,
    small: bool,
    key_context: Option<SharedString>,
    on_click: Option<Action>,
}

impl std::fmt::Debug for Button {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Button")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("variant", &self.variant)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

impl Button {
    /// An outlined button reading `label`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            variant: Variant::Outline,
            tokens,
            focus: None,
            disabled: None,
            tooltip: None,
            shortcut: Vec::new(),
            icon: None,
            icon_only: false,
            small: false,
            key_context: None,
            on_click: None,
        }
    }

    /// The button drawn as `variant`.
    #[must_use]
    pub fn variant(mut self, variant: Variant) -> Self {
        self.variant = variant;
        self
    }

    /// The handle the button takes focus through, which makes it a tab stop
    /// and shows its ring for keyboard focus.
    #[must_use]
    pub fn focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    /// Disables the button; `reason`, when given, shows as its tooltip.
    #[must_use]
    pub fn disabled(mut self, reason: Option<SharedString>) -> Self {
        self.disabled = Some(reason.unwrap_or_default());
        self
    }

    /// A word of explanation shown on hover, such as an icon button's name.
    #[must_use]
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// The shortcut drawn as a key cap after the label.
    #[must_use]
    pub fn shortcut(mut self, glyphs: Vec<Glyph>) -> Self {
        self.shortcut = glyphs;
        self
    }

    /// An icon drawn before the label, such as an SVG at the text size.
    #[must_use]
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    /// Draws the icon alone in a control-height square; the label stays
    /// the accessible name and becomes the tooltip, unless one is set. The
    /// shortcut stays accessible but draws no key cap, which the square has
    /// no room for. It needs an icon, and a link has no icon-only form.
    #[must_use]
    pub fn icon_only(mut self) -> Self {
        self.icon_only = true;
        self
    }

    /// Draws the button at [`size::SMALL_CONTROL_HEIGHT`], to fit inside
    /// another control such as an input group.
    #[must_use]
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// The key context the button carries, such as the owner's control
    /// context that its activation command binds under.
    #[must_use]
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    /// Called for a pointer click.
    #[must_use]
    pub fn on_click(
        mut self,
        action: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(action));
        self
    }
}

/// A variant's fill, text, border, hover, and pressed colors.
struct Look {
    fill: Option<Hsla>,
    text: Hsla,
    border: Option<Hsla>,
    hover: Hsla,
    pressed: Hsla,
}

fn look(variant: Variant, tokens: Tokens) -> Look {
    match variant {
        Variant::Outline => Look {
            fill: None,
            text: tokens.fg,
            border: Some(tokens.line_strong),
            hover: tokens.hover,
            pressed: tokens.pressed(),
        },
        Variant::Secondary => Look {
            fill: Some(tokens.chip),
            text: tokens.fg,
            border: None,
            hover: tokens.chip_hover(),
            pressed: tokens.pressed(),
        },
        Variant::Ghost => Look {
            fill: None,
            text: tokens.fg,
            border: None,
            hover: tokens.hover,
            pressed: tokens.pressed(),
        },
        Variant::Primary => Look {
            fill: Some(tokens.accent),
            text: tokens.on_accent(),
            border: None,
            hover: tokens.shade(tokens.accent, 0.1),
            pressed: tokens.accent_pressed(),
        },
        Variant::Destructive => Look {
            fill: Some(tokens.danger),
            text: tokens.on(tokens.danger),
            border: None,
            // Shades move away from the text, so it reads in every state.
            hover: tokens.shade(tokens.danger, 0.1),
            pressed: tokens.shade(tokens.danger, 0.2),
        },
        // A link underlines on hover instead of filling.
        Variant::Link => Look {
            fill: None,
            text: tokens.accent_text(),
            border: None,
            hover: tokens.hover,
            pressed: tokens.pressed(),
        },
    }
}

/// `button`'s hover and pressed states, or, when disabled, its dimming and
/// a press guard: a disabled control keeps its handle but takes no focus
/// from a press.
fn interactions(
    button: Stateful<Div>,
    variant: Variant,
    look: &Look,
    enabled: bool,
) -> Stateful<Div> {
    if !enabled {
        return button.opacity(0.5).on_mouse_down(
            MouseButton::Left,
            |_, window, _| {
                window.prevent_default();
            },
        );
    }
    if variant == Variant::Link {
        return button.hover(Styled::underline);
    }
    let (hover, pressed) = (look.hover, look.pressed);
    button
        .hover(move |style| style.bg(hover))
        .active(move |style| style.bg(pressed))
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let look = look(self.variant, tokens);
        let description = self
            .disabled
            .clone()
            .filter(|reason| !reason.is_empty())
            .or(self.tooltip.clone());
        debug_assert!(
            !self.icon_only
                || (self.icon.is_some() && self.variant != Variant::Link),
            "an icon-only button needs an icon and is not a link"
        );
        // The cap draws outlined in the label's color, so it reads on
        // every fill the button takes.
        let cap = (!self.shortcut.is_empty() && !self.icon_only)
            .then(|| kbd::cap_on(&self.shortcut, look.text));
        let keyshortcuts =
            (!self.shortcut.is_empty()).then(|| kbd::text(&self.shortcut));
        // An icon-only button names itself in its tooltip; the label is
        // already its accessible name, so it is not also its description.
        let tooltip = description
            .clone()
            .or_else(|| self.icon_only.then(|| self.label.clone()));
        let icon_only = self.icon_only;
        let height = if self.small {
            size::SMALL_CONTROL_HEIGHT
        } else {
            size::CONTROL_HEIGHT
        };
        let focus = self.focus.map(|focus| focus.tab_stop(enabled));
        let button = div()
            .id(self.id)
            .role(if self.variant == Variant::Link {
                gpui::Role::Link
            } else {
                gpui::Role::Button
            })
            .aria_label(self.label.clone())
            .when_some(description.clone(), |button, text| {
                button.aria_description(text)
            })
            .when_some(keyshortcuts, |button, text| {
                button.aria_keyshortcuts(text)
            })
            .when_some(focus.as_ref(), |button, focus| {
                button.track_focus(focus)
            })
            .when_some(self.key_context, |button, context| {
                button.key_context(context.as_ref())
            })
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(height)
            .when(self.variant != Variant::Link, |button| {
                button.rounded(px(size::CONTROL_RADIUS))
            })
            .when(self.variant != Variant::Link && !icon_only, |button| {
                button.px(px(if self.small { 6.0 } else { 10.0 }))
            })
            .when(icon_only, |button| button.w(height).justify_center())
            .text_size(size::TEXT_CONTROL)
            .text_color(look.text)
            .when_some(look.fill, Styled::bg)
            .when_some(look.border, |button, border| {
                button.border_1().border_color(border)
            })
            .when_some(tooltip, |button, text| {
                button.tooltip(move |_, cx| {
                    Tooltip::view(text.clone(), tokens, cx)
                })
            })
            .map(|button| interactions(button, self.variant, &look, enabled))
            .when_some(self.on_click.filter(|_| enabled), |button, action| {
                button.on_click(move |event, window, cx| {
                    if !from_keyboard(event) {
                        action(window, cx);
                    }
                })
            })
            .children(self.icon)
            .when(!icon_only, |button| button.child(self.label))
            .children(cap);
        let button = match &focus {
            Some(focus) => focus::ring(button, focus, tokens, window, cx),
            None => button,
        };
        States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(button)
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

    use super::{Button, Variant};
    use crate::components::kbd::Glyph;
    use crate::focus::ring_drawn;
    use crate::testing::{a11y_node, host, tokens};

    struct Probe {
        focus: FocusHandle,
        clicks: Rc<Cell<usize>>,
    }

    fn button(
        cx: &mut TestAppContext,
        build: impl Fn(Button) -> Button + 'static,
    ) -> (Probe, &mut VisualTestContext) {
        let focus = cx.update(|cx| cx.focus_handle());
        let clicks = Rc::new(Cell::new(0));
        let (drawn, counted) = (focus.clone(), clicks.clone());
        let (_, cx) = host(cx, move |_, _| {
            let counted = counted.clone();
            build(
                Button::new("reload", "Reload", tokens())
                    .focus(&drawn)
                    .on_click(move |_, _| counted.set(counted.get() + 1)),
            )
            .into_any_element()
        });
        (Probe { focus, clicks }, cx)
    }

    #[gpui::test]
    fn a_button_reports_its_name_shortcut_and_tooltip(cx: &mut TestAppContext) {
        let (_, cx) = button(cx, |button| {
            button.tooltip("Reload the config file").shortcut(vec![
                Glyph::Text("⌘".into()),
                Glyph::Text("R".into()),
            ])
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "reload")["aria"];
            assert_eq!(aria["role"], "Button");
            assert_eq!(aria["label"], "Reload");
            assert_eq!(aria["keyboard_shortcut"], "⌘R");
            assert_eq!(aria["description"], "Reload the config file");
            assert!(aria["disabled"].is_null());
        });
    }

    #[gpui::test]
    fn a_link_button_reports_a_link(cx: &mut TestAppContext) {
        let (_, cx) = button(cx, |button| button.variant(Variant::Link));
        cx.update(|window, _| {
            assert_eq!(a11y_node(window, "reload")["aria"]["role"], "Link");
        });
    }

    #[gpui::test]
    fn a_pointer_click_acts_and_an_implicit_key_click_does_not(
        cx: &mut TestAppContext,
    ) {
        let (probe, cx) = button(cx, |button| button);
        cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(probe.clicks.get(), 1);
        assert!(cx.update(|window, _| probe.focus.is_focused(window)));
        // GPUI clicks a focused element on Space's key-up; nothing binds
        // Space here, so only that implicit click could act.
        cx.simulate_keystrokes("space");
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("space").expect("a valid keystroke"),
        });
        cx.run_until_parked();
        assert_eq!(probe.clicks.get(), 1);
    }

    #[gpui::test]
    fn a_disabled_button_neither_acts_nor_takes_focus(cx: &mut TestAppContext) {
        let (probe, cx) = button(cx, |button| {
            button.disabled(Some("The file is invalid".into()))
        });
        cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(probe.clicks.get(), 0);
        assert!(!cx.update(|window, _| probe.focus.is_focused(window)));
        cx.update(|window, _| {
            let aria = &a11y_node(window, "reload")["aria"];
            assert_eq!(aria["disabled"], true);
            assert_eq!(aria["description"], "The file is invalid");
        });
    }

    #[gpui::test]
    fn keyboard_focus_draws_the_ring(cx: &mut TestAppContext) {
        let (probe, cx) = button(cx, |button| button);
        cx.simulate_keystrokes("a");
        cx.update(|window, cx| probe.focus.focus(window, cx));
        cx.run_until_parked();
        assert!(cx.update(|window, cx| ring_drawn(&probe.focus, window, cx)));
    }

    #[gpui::test]
    fn an_icon_only_button_keeps_its_name(cx: &mut TestAppContext) {
        let (_, cx) = button(cx, |button| {
            button.icon(gpui::div().size(px(14.0))).icon_only()
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "reload")["aria"];
            assert_eq!(aria["label"], "Reload");
            // The label is its name, so it is not repeated as a description.
            assert!(aria["description"].is_null());
        });
    }

    #[gpui::test]
    fn an_icon_only_button_keeps_its_icon_inside_and_its_shortcut_named(
        cx: &mut TestAppContext,
    ) {
        let (_, cx) = button(cx, |button| {
            button
                .icon(
                    gpui::div()
                        .size(px(14.0))
                        .debug_selector(|| "icon".to_owned()),
                )
                .icon_only()
                .shortcut(vec![Glyph::Text("N".into())])
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "reload")["aria"];
            assert_eq!(aria["keyboard_shortcut"], "N");
        });
        // The square has no room for a key cap, so the icon stays inside.
        let icon = cx.debug_bounds("icon").expect("the icon drew");
        assert!(icon.origin.x >= px(0.0), "{icon:?}");
        assert!(icon.origin.x + icon.size.width <= px(24.0), "{icon:?}");
    }
}
