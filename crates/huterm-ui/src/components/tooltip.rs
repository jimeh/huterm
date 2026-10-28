//! Tooltip: a one-line explanation shown while the pointer rests on a
//! control or keyboard focus is on it, such as an icon button's name or a
//! disabled control's reason.
//!
//! Anatomy: short text on the raised surface color with a border and the
//! drop shadow. GPUI places and times the hover tooltip: attach it with a
//! control's `tooltip` builder and [`Tooltip::view`]. GPUI draws tooltips in
//! their own layer, so showing one never closes a popover.
//!
//! Behavior: GPUI shows it after its hover delay and hides it when the
//! pointer leaves. A control that keyboard focus reached shows it at once,
//! below itself, while its focus ring shows: mount [`focused`] in it then.
//! A pointer press hides the ring, and the tooltip with it.
//!
//! Accessibility: the tooltip view reports no node. The control it explains
//! carries the text as its `aria_description`, because assistive technology
//! does not hover.

use std::rc::Rc;

use gpui::{
    AnyTooltip, AnyView, App, Context, IntoElement, Render, SharedString,
    Window, canvas, div, point, prelude::*, px,
};

use crate::tokens::Tokens;

/// The gap between a focused control and the tooltip below it.
const FOCUS_GAP: f32 = 4.0;

/// The tooltip view GPUI's `tooltip` builder expects.
#[derive(Debug)]
pub struct Tooltip {
    text: SharedString,
    tokens: Tokens,
}

impl Tooltip {
    /// A tooltip reading `text`.
    pub fn view(
        text: impl Into<SharedString>,
        tokens: Tokens,
        cx: &mut App,
    ) -> AnyView {
        let text = text.into();
        cx.new(|_| Self { text, tokens }).into()
    }
}

/// Shows `text` as a tooltip below its parent in this frame, for a control
/// that shows its keyboard focus ring. Mount it in the control only while
/// the ring shows; it takes no space. GPUI keeps the tooltip inside the
/// window.
#[must_use]
pub fn focused(text: SharedString, tokens: Tokens) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            let view = Tooltip::view(text, tokens, cx);
            window.set_tooltip(AnyTooltip {
                view,
                // GPUI draws a tooltip just past this point, as it does
                // past the pointer.
                mouse_position: point(
                    bounds.left(),
                    bounds.bottom() + px(FOCUS_GAP),
                ),
                check_visible_and_update: Rc::new(|_, _, _| true),
            });
        },
        |_, (), _, _| {},
    )
    .absolute()
    .inset_0()
}

impl Render for Tooltip {
    fn render(
        &mut self,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let tokens = self.tokens;
        div()
            .debug_selector(|| "tooltip".to_owned())
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(tokens.surface)
            .border_1()
            .border_color(tokens.line_strong)
            .shadow(tokens.shadow())
            .text_size(px(11.5))
            .text_color(tokens.fg)
            .whitespace_nowrap()
            .child(self.text.clone())
    }
}
