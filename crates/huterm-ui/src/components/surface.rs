//! Surface: the raised panel menus, dialogs, toasts, and the palette sit on.
//!
//! Anatomy: the tokens' surface fill, a one-point `line_strong` border, the
//! two-layer drop shadow, and a corner radius, clipping its content.
//!
//! Behavior: none of its own. A surface is a container; the component built
//! on it supplies interaction, and occludes what it covers when it must
//! stop presses.
//!
//! Accessibility: a surface reports no node. The component built on it
//! gives the role, such as a menu or a dialog.

use gpui::{Div, div, prelude::*, px};

use crate::tokens::Tokens;

/// A raised surface with a `radius` corner, such as
/// [`crate::tokens::size::PANEL_RADIUS`].
#[must_use]
pub fn surface(tokens: Tokens, radius: f32) -> Div {
    div()
        .bg(tokens.surface)
        .border_1()
        .border_color(tokens.line_strong)
        .rounded(px(radius))
        .shadow(tokens.shadow())
        .overflow_hidden()
}
