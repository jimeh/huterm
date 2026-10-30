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
//! below itself, while its focus ring shows and it is not scrolled out of
//! view: mount [`focused`] in it then. A pointer press hides the ring, and
//! the tooltip with it.
//!
//! Accessibility: the tooltip view reports no node. The control it explains
//! carries the text as its `aria_description`, because assistive technology
//! does not hover.

use std::rc::Rc;

use gpui::{
    AnyTooltip, AnyView, App, Context, IntoElement, Render, SharedString,
    Window, canvas, div, point, prelude::*, px,
};

use crate::tokens::{Tokens, size};
use crate::typography::Typography;

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
/// window. A control clipped by its scroll area, even in part, shows none,
/// so the tooltip never floats over what covers the control.
#[must_use]
pub fn focused(text: SharedString, tokens: Tokens) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            if window.content_mask().bounds.intersect(&bounds) != bounds {
                return;
            }
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
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let tokens = self.tokens;
        // GPUI lays tooltips out outside the window's tree, so the text
        // inherits no family from its root.
        div()
            .debug_selector(|| "tooltip".to_owned())
            .font_family(Typography::of(cx).family)
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(tokens.surface)
            .border_1()
            .border_color(tokens.line_strong)
            .shadow(tokens.shadow())
            .text_size(size::TEXT_SMALL)
            .text_color(tokens.fg)
            .whitespace_nowrap()
            .child(self.text.clone())
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{
        Context, FocusHandle, IntoElement, Render, ScrollHandle,
        TestAppContext, Window, div, point, prelude::*, px,
    };

    use crate::components::button::Button;
    use crate::components::scroll_area::ScrollArea;
    use crate::list_scrollbar::ListScrollbar;
    use crate::testing::tokens;

    /// A focusable button with a tooltip at the top of a short scroll area.
    struct Scrolled {
        focus: FocusHandle,
        scroll: ScrollHandle,
        scrollbar: ListScrollbar,
    }

    impl Render for Scrolled {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut Context<'_, Self>,
        ) -> impl IntoElement {
            let view = cx.entity();
            div().size(px(400.0)).child(crate::focus::tracker()).child(
                ScrollArea::new("area", &self.scroll)
                    .max_height(Some(px(100.0)))
                    .child(
                        Button::new("reload", "Reload", tokens())
                            .focus(&self.focus)
                            .tooltip("Reload the config file"),
                    )
                    .children((0..20).map(|_| div().flex_none().h(px(20.0))))
                    .render(
                        &mut self.scrollbar,
                        tokens().scrollbar,
                        &view,
                        |view| &mut view.scrollbar,
                        cx,
                    ),
            )
        }
    }

    #[gpui::test]
    fn a_focused_control_scrolled_out_of_view_shows_no_tooltip(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            let scroll = ScrollHandle::new();
            Scrolled {
                focus: cx.focus_handle(),
                scrollbar: ListScrollbar::new(scroll.clone()),
                scroll,
            }
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("a");
        cx.update(|window, cx| {
            view.read(cx).focus.clone().focus(window, cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("tooltip").is_some(), "in view, it shows");
        view.update(cx, |view, cx| {
            view.scroll.set_offset(point(px(0.0), px(-60.0)));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.update(|window, cx| view.read(cx).focus.is_focused(window)));
        assert!(
            cx.debug_bounds("tooltip").is_none(),
            "scrolled out, it hides"
        );
    }
}
