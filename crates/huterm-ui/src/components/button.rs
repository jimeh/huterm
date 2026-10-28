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
//! link. A button drawn on another surface's palette, such as a tab bar's,
//! takes its [`Colors`] instead; its owner may also resize, place, and
//! restyle it through `Styled`, which applies over the variant's style, and
//! add children, such as a badge, after its content.
//!
//! Keyboard: a button the owner gives a focus handle takes focus; the owner
//! runs its action on its activation command (Space and Return by default).
//! GPUI's implicit keyboard click is ignored, so only that routed command
//! acts on keys. An owner that keeps focus itself and moves between its
//! buttons with its own commands, as a dialog does, marks the keyboard's
//! place with [`Button::current`] instead.
//!
//! Pointer: a click runs the action; hover and pressed states show. A
//! disabled button takes no focus from a press and does not act, and shows
//! its reason as a tooltip.
//!
//! Focus: the ring shows for keyboard focus, or on the current button while
//! the window's last press was a key, never after a pointer press. A button
//! with a tooltip shows it below itself while the ring shows.
//!
//! Accessibility: a `Button` (or `Link`) named by its label, reporting its
//! shortcut, its tooltip or disabled reason as its description, whether a
//! menu it opens is expanded, and its disabled state.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, ElementId, FocusHandle, Hsla, IntoElement,
    MouseButton, ParentElement, Refineable, RenderOnce, SharedString, Stateful,
    StyleRefinement, Styled, Window, div, prelude::*, px,
};

use crate::a11y::States;
use crate::click::from_keyboard;
use crate::components::kbd::{self, Glyph};
use crate::components::tooltip::{self, Tooltip};
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

/// A button's fill, text, border, hover, and pressed colors. A link
/// underlines on hover instead of filling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    /// The resting fill, if any.
    pub fill: Option<Hsla>,
    /// The label, icon tint, and key cap color.
    pub text: Hsla,
    /// The one-point border, if any.
    pub border: Option<Hsla>,
    /// The fill while hovered.
    pub hover: Hsla,
    /// The fill while held down.
    pub pressed: Hsla,
}

type Action = Rc<dyn Fn(&mut Window, &mut App)>;

/// A button.
#[derive(IntoElement)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent builder option"
)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: Variant,
    tokens: Tokens,
    colors: Option<Colors>,
    focus: Option<FocusHandle>,
    current: bool,
    disabled: Option<SharedString>,
    tooltip: Option<SharedString>,
    shortcut: Vec<Glyph>,
    icon: Option<AnyElement>,
    icon_only: bool,
    small: bool,
    expanded: Option<bool>,
    group: Option<SharedString>,
    occlude: bool,
    stop_presses: bool,
    key_context: Option<SharedString>,
    debug_selector: Option<String>,
    on_click: Option<Action>,
    style: StyleRefinement,
    children: Vec<AnyElement>,
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
            colors: None,
            focus: None,
            current: false,
            disabled: None,
            tooltip: None,
            shortcut: Vec::new(),
            icon: None,
            icon_only: false,
            small: false,
            expanded: None,
            group: None,
            occlude: false,
            stop_presses: false,
            key_context: None,
            debug_selector: None,
            on_click: None,
            style: StyleRefinement::default(),
            children: Vec::new(),
        }
    }

    /// The button drawn as `variant`.
    #[must_use]
    pub fn variant(mut self, variant: Variant) -> Self {
        self.variant = variant;
        self
    }

    /// The button drawn in `colors` rather than its variant's, for a button
    /// on another surface's palette, such as a tab bar's.
    #[must_use]
    pub fn colors(mut self, colors: Colors) -> Self {
        self.colors = Some(colors);
        self
    }

    /// The handle the button takes focus through, which makes it a tab stop
    /// and shows its ring for keyboard focus.
    #[must_use]
    pub fn focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    /// Marks the button as the keyboard's place among its owner's buttons,
    /// for an owner that keeps focus itself and moves between them with its
    /// own commands, as a dialog does. It shows the ring while the window's
    /// last press was a key.
    #[must_use]
    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
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

    /// Reports whether the menu the button opens is expanded.
    #[must_use]
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    /// Names the button as a hover group, so an icon passed to it can
    /// change with the button's hover through `group_hover`.
    #[must_use]
    pub fn group(mut self, name: impl Into<SharedString>) -> Self {
        self.group = Some(name.into());
        self
    }

    /// Keeps the pointer's hover and presses from reaching what the button
    /// covers.
    #[must_use]
    pub fn occlude(mut self) -> Self {
        self.occlude = true;
        self
    }

    /// Stops primary presses at the button, so they never reach what it
    /// sits on, such as a title row whose press moves the window.
    #[must_use]
    pub fn stop_presses(mut self) -> Self {
        self.stop_presses = true;
        self
    }

    /// The key context the button carries, such as the owner's control
    /// context that its activation command binds under.
    #[must_use]
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    /// Names the button for GPUI's `debug_bounds` in tests; GPUI ignores it
    /// elsewhere.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<String>) -> Self {
        self.debug_selector = Some(selector.into());
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

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

fn colors(variant: Variant, tokens: Tokens) -> Colors {
    match variant {
        Variant::Outline => Colors {
            fill: None,
            text: tokens.fg,
            border: Some(tokens.line_strong),
            hover: tokens.hover,
            pressed: tokens.pressed(),
        },
        Variant::Secondary => Colors {
            fill: Some(tokens.chip),
            text: tokens.fg,
            border: None,
            hover: tokens.chip_hover(),
            pressed: tokens.pressed(),
        },
        Variant::Ghost => Colors {
            fill: None,
            text: tokens.fg,
            border: None,
            hover: tokens.hover,
            pressed: tokens.pressed(),
        },
        Variant::Primary => Colors {
            fill: Some(tokens.accent),
            text: tokens.on_accent(),
            border: None,
            hover: tokens.shade(tokens.accent, 0.1),
            pressed: tokens.accent_pressed(),
        },
        Variant::Destructive => Colors {
            fill: Some(tokens.danger),
            text: tokens.on(tokens.danger),
            border: None,
            // Shades move away from the text, so it reads in every state.
            hover: tokens.shade(tokens.danger, 0.1),
            pressed: tokens.shade(tokens.danger, 0.2),
        },
        // A link underlines on hover instead of filling.
        Variant::Link => Colors {
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
    colors: &Colors,
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
    let (hover, pressed) = (colors.hover, colors.pressed);
    button
        .hover(move |style| style.bg(hover))
        .active(move |style| style.bg(pressed))
}

impl Button {
    /// The button's own element: its identity, accessibility, layout,
    /// colors, and the owner's style over them, before its states, content,
    /// and ring.
    fn frame(
        &mut self,
        look: &Colors,
        description: Option<SharedString>,
        focus: Option<&FocusHandle>,
    ) -> Stateful<Div> {
        let height = if self.small {
            size::SMALL_CONTROL_HEIGHT
        } else {
            size::CONTROL_HEIGHT
        };
        let link = self.variant == Variant::Link;
        let keyshortcuts =
            (!self.shortcut.is_empty()).then(|| kbd::text(&self.shortcut));
        let mut button = div()
            .id(self.id.clone())
            .role(if link {
                gpui::Role::Link
            } else {
                gpui::Role::Button
            })
            .aria_label(self.label.clone())
            .when_some(description, |button, text| {
                button.aria_description(text)
            })
            .when_some(keyshortcuts, |button, text| {
                button.aria_keyshortcuts(text)
            })
            .when_some(self.expanded, |button, expanded| {
                button.aria_expanded(expanded)
            })
            .when_some(focus, InteractiveElement::track_focus)
            .when_some(self.key_context.take(), |button, context| {
                button.key_context(context.as_ref())
            })
            .when_some(self.group.take(), InteractiveElement::group)
            .when_some(self.debug_selector.take(), |button, selector| {
                button.debug_selector(|| selector)
            })
            .when(self.occlude, InteractiveElement::occlude)
            .when(self.stop_presses, |button| {
                button.on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
            })
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(height)
            .when(!link, |button| button.rounded(px(size::CONTROL_RADIUS)))
            .when(!link && !self.icon_only, |button| {
                button.px(px(if self.small { 6.0 } else { 10.0 }))
            })
            .when(self.icon_only, |button| button.w(height).justify_center())
            .text_size(size::TEXT_CONTROL)
            .text_color(look.text)
            .when_some(look.fill, Styled::bg)
            .when_some(look.border, |button, border| {
                button.border_1().border_color(border)
            });
        // The owner's style applies over the variant's.
        button.style().refine(&std::mem::take(&mut self.style));
        button
    }
}

impl RenderOnce for Button {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let look = self.colors.unwrap_or_else(|| colors(self.variant, tokens));
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
        // An icon-only button names itself in its tooltip; the label is
        // already its accessible name, so it is not also its description.
        let tooltip = description
            .clone()
            .or_else(|| self.icon_only.then(|| self.label.clone()));
        let focus = self.focus.take().map(|focus| focus.tab_stop(enabled));
        let ringed = match &focus {
            Some(focus) => focus::visible(focus, window, cx),
            None => self.current && focus::current_visible(window, cx),
        };
        let button = self
            .frame(&look, description, focus.as_ref())
            .when_some(tooltip.clone(), |button, text| {
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
            .when(!self.icon_only, |button| button.child(self.label))
            .children(cap)
            .children(self.children)
            .when_some(tooltip.filter(|_| ringed), |button, text| {
                button.child(tooltip::focused(text, tokens))
            });
        let button = match &focus {
            Some(focus) => focus::ring(button, focus, tokens, window, cx),
            None if self.current => {
                focus::current_ring(button, &self.id, tokens, window, cx)
            }
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
    use crate::focus::{current_ring_drawn, ring_drawn};
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

    #[gpui::test]
    fn keyboard_focus_shows_the_tooltip_until_a_press(cx: &mut TestAppContext) {
        let (probe, cx) =
            button(cx, |button| button.tooltip("Reload the config file"));
        assert!(cx.debug_bounds("tooltip").is_none());
        cx.simulate_keystrokes("a");
        cx.update(|window, cx| probe.focus.focus(window, cx));
        cx.run_until_parked();
        let tooltip = cx
            .debug_bounds("tooltip")
            .expect("keyboard focus shows the tooltip");
        assert!(
            tooltip.origin.y >= px(24.0),
            "below the button: {tooltip:?}"
        );
        cx.update(|window, _| {
            let aria = &a11y_node(window, "reload")["aria"];
            assert_eq!(aria["description"], "Reload the config file");
        });
        cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert!(cx.update(|window, _| probe.focus.is_focused(window)));
        assert!(cx.debug_bounds("tooltip").is_none(), "a press hides it");
    }

    #[gpui::test]
    fn a_current_button_shows_its_ring_after_a_key_and_not_after_a_press(
        cx: &mut TestAppContext,
    ) {
        let (_, cx) = host(cx, |_, _| {
            Button::new("cancel", "Cancel", tokens())
                .current(true)
                .into_any_element()
        });
        let drawn = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| current_ring_drawn("cancel", window, cx))
        };
        // Nothing binds the key, so only the focus record redraws.
        let key = |cx: &mut VisualTestContext| {
            cx.simulate_keystrokes("a");
            cx.run_until_parked();
        };
        key(cx);
        assert!(drawn(cx), "a key shows the ring");
        cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert!(!drawn(cx), "a press hides it");
        key(cx);
        assert!(drawn(cx), "the next key shows it again");
    }

    #[gpui::test]
    fn an_owner_restyles_the_button_and_adds_children(cx: &mut TestAppContext) {
        let (_, cx) = button(cx, |button| {
            button
                .icon(gpui::div().size(px(14.0)))
                .icon_only()
                .expanded(true)
                .debug_selector("reload")
                .w(px(30.0))
                .h(px(30.0))
                .child(
                    gpui::div()
                        .absolute()
                        .size(px(4.0))
                        .debug_selector(|| "badge".to_owned()),
                )
        });
        let bounds = cx.debug_bounds("reload").expect("the button drew");
        assert_eq!(bounds.size, gpui::size(px(30.0), px(30.0)));
        assert!(cx.debug_bounds("badge").is_some());
        cx.update(|window, _| {
            assert_eq!(a11y_node(window, "reload")["aria"]["expanded"], true);
        });
    }
}
