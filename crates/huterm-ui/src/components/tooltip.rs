//! Tooltip: a one-line explanation shown while the pointer rests on a
//! control, such as an icon button's name or a disabled control's reason.
//!
//! Anatomy: short text on the raised surface color with a border and the
//! drop shadow. GPUI places and times it: attach it with a control's
//! `tooltip` builder and [`Tooltip::view`]. GPUI draws tooltips in their own
//! layer, so showing one never closes a popover.
//!
//! Behavior: GPUI shows it after its hover delay and hides it when the
//! pointer leaves. Showing it for keyboard focus is a later step.
//!
//! Accessibility: the tooltip view reports a `Tooltip` node named by its
//! text. The control it explains should also carry the text as its
//! `aria_description`, because assistive technology does not hover.

use gpui::{
    AnyView, App, Context, IntoElement, Render, SharedString, Window, div,
    prelude::*, px,
};

use crate::tokens::Tokens;

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

impl Render for Tooltip {
    fn render(
        &mut self,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let tokens = self.tokens;
        div()
            .id("huterm-ui-tooltip")
            .role(gpui::Role::Tooltip)
            .aria_label(self.text.clone())
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

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{TestAppContext, prelude::*};

    use super::Tooltip;
    use crate::testing::{a11y_node, host, tokens};

    #[gpui::test]
    fn a_tooltip_reads_as_its_text(cx: &mut TestAppContext) {
        let (_, cx) = host(cx, |_, cx| {
            Tooltip::view("New tab", tokens(), cx).into_any_element()
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "huterm-ui-tooltip")["aria"];
            assert_eq!(aria["role"], "Tooltip");
            assert_eq!(aria["label"], "New tab");
        });
    }
}
