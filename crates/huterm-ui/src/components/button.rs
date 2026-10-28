//! Button: a labelled control that runs an action, such as Reset, Open
//! config.toml, or a dialog's Cancel and Delete.
//!
//! Anatomy: a label, optionally followed by a key cap for its shortcut, in a
//! control-height pill. [`Variant`]s follow shadcn/ui's: outlined by
//! default, a quiet secondary fill, a borderless ghost, an accent fill for
//! the primary action, a danger fill for destructive actions, and a link.
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
    App, ElementId, FocusHandle, Hsla, IntoElement, MouseButton, RenderOnce,
    SharedString, Window, div, prelude::*, px,
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
            hover: tokens.pressed(),
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
            hover: tokens.accent_pressed(),
            pressed: tokens.accent_pressed(),
        },
        Variant::Destructive => Look {
            fill: Some(tokens.danger),
            text: tokens.bg,
            border: None,
            hover: crate::tokens::mix(tokens.danger, tokens.bg, 0.15),
            pressed: crate::tokens::mix(tokens.danger, tokens.bg, 0.22),
        },
        // A link underlines on hover instead of filling.
        Variant::Link => Look {
            fill: None,
            text: tokens.accent,
            border: None,
            hover: tokens.hover,
            pressed: tokens.pressed(),
        },
    }
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
        let filled =
            matches!(self.variant, Variant::Primary | Variant::Destructive);
        let cap = (!self.shortcut.is_empty()).then(|| {
            if filled {
                kbd::cap_on(&self.shortcut, look.text)
            } else {
                kbd::cap(&self.shortcut, tokens)
            }
        });
        let keyshortcuts =
            (!self.shortcut.is_empty()).then(|| kbd::text(&self.shortcut));
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
            .h(size::CONTROL_HEIGHT)
            .when(self.variant != Variant::Link, |button| {
                button.px(px(10.0)).rounded(px(size::CONTROL_RADIUS))
            })
            .text_size(size::TEXT_CONTROL)
            .text_color(look.text)
            .when_some(look.fill, Styled::bg)
            .when_some(look.border, |button, border| {
                button.border_1().border_color(border)
            })
            .when_some(description, |button, text| {
                button.tooltip(move |_, cx| {
                    Tooltip::view(text.clone(), tokens, cx)
                })
            })
            .when(enabled, |button| {
                if self.variant == Variant::Link {
                    button.hover(Styled::underline)
                } else {
                    button
                        .hover(|style| style.bg(look.hover))
                        .active(|style| style.bg(look.pressed))
                }
            })
            .when(!enabled, |button| {
                // A disabled control keeps its handle but takes no focus
                // from a press.
                button.opacity(0.5).on_mouse_down(
                    MouseButton::Left,
                    |_, window, _| {
                        window.prevent_default();
                    },
                )
            })
            .when_some(self.on_click.filter(|_| enabled), |button, action| {
                button.on_click(move |event, window, cx| {
                    if !from_keyboard(event) {
                        action(window, cx);
                    }
                })
            })
            .child(self.label)
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
}
