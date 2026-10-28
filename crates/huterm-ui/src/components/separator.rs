//! Separator: a hairline between groups of content, such as menu sections.
//!
//! Anatomy: a one-point line in the tokens' `line` color, stretching across
//! a flex column (horizontal) or row (vertical), as flex children do by
//! default. Callers add the margins their layout needs.
//!
//! Behavior: none; it never takes focus or input.
//!
//! Accessibility: decorative, so it reports no node. A list or menu that
//! groups items reports the grouping through its own structure.

use gpui::{Div, div, prelude::*, px};

use crate::tokens::Tokens;

/// A horizontal hairline across the container.
#[must_use]
pub fn horizontal(tokens: Tokens) -> Div {
    div().flex_none().h(px(1.0)).bg(tokens.line)
}

/// A vertical hairline down the container.
#[must_use]
pub fn vertical(tokens: Tokens) -> Div {
    div().flex_none().w(px(1.0)).bg(tokens.line)
}
