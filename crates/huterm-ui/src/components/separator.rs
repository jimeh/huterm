//! Separator: a hairline between groups of content, such as menu sections.
//!
//! Anatomy: a one-point line in the tokens' `line` color, stretching across
//! its flex column (horizontal) or row (vertical) whatever the container's
//! alignment, less its margins. Callers add the margins their layout needs.
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
    div().flex_none().self_stretch().h(px(1.0)).bg(tokens.line)
}

/// A vertical hairline down the container.
#[must_use]
pub fn vertical(tokens: Tokens) -> Div {
    div().flex_none().self_stretch().w(px(1.0)).bg(tokens.line)
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{TestAppContext, div, prelude::*, px};

    use super::{horizontal, vertical};
    use crate::testing::{host, tokens};

    #[gpui::test]
    fn separators_span_containers_that_center_their_items(
        cx: &mut TestAppContext,
    ) {
        let (_, cx) = host(cx, |_, _| {
            div()
                .child(div().flex().items_center().h(px(20.0)).child(
                    vertical(tokens()).debug_selector(|| "vertical".into()),
                ))
                .child(
                    div().flex().flex_col().items_center().w(px(100.0)).child(
                        horizontal(tokens())
                            .mx(px(4.0))
                            .debug_selector(|| "horizontal".into()),
                    ),
                )
                .into_any_element()
        });
        let vertical = cx.debug_bounds("vertical").expect("drawn");
        assert_eq!(vertical.size.height, px(20.0));
        let horizontal = cx.debug_bounds("horizontal").expect("drawn");
        assert_eq!(horizontal.size.width, px(92.0));
    }
}
