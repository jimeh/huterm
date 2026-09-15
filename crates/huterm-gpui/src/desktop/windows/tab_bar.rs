//! Theme-derived tab bar colors and tab item presentation.

use gpui::{Div, FontWeight, Hsla, Stateful, Svg, svg};
use huterm_config::TabStyle;

use crate::assets::Icon;

use super::{
    Bounds, CloseTarget, Context, FluentBuilder, InteractiveElement,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Presentation,
    StatefulInteractiveElement, Styled, TabId, TabPosition, Theme,
    WorkspaceView, color, div, px,
};

const PILL_HEIGHT: Pixels = px(26.0);

#[derive(Clone, Copy)]
pub(super) struct TabColors {
    pub(super) bar: Hsla,
    pub(super) active: Hsla,
    pub(super) foreground: Hsla,
    pub(super) inactive: Hsla,
    pub(super) border: Hsla,
    pub(super) accent: Hsla,
    pub(super) terminal: Hsla,
    pub(super) error: Hsla,
}

impl TabColors {
    pub(super) fn new(theme: &Theme) -> Self {
        let ui = theme.ui();
        Self {
            bar: color(ui.tab_bar_background),
            active: color(ui.tab_active_background),
            foreground: color(ui.tab_foreground),
            inactive: color(ui.tab_inactive_foreground),
            border: color(ui.tab_border),
            accent: color(ui.tab_accent),
            terminal: color(theme.background),
            error: color(theme.ansi[1]),
        }
    }

    fn hover(self) -> Hsla {
        self.foreground.opacity(0.04)
    }
}

/// A 12-point Lucide icon. SVGs paint with their own text color, not an
/// inherited one, so hover changes must target the icon through a group.
pub(super) fn icon_element(icon: Icon, tint: Hsla) -> Svg {
    svg()
        .path(icon.asset_path())
        .w(px(12.0))
        .h(px(12.0))
        .flex_shrink_0()
        .text_color(tint)
}

/// Whether the region above the tab bar and terminal (the macOS titlebar or
/// the display safe area above a notch) uses the tab bar background.
pub(super) fn top_chrome_uses_bar(
    presentation: Presentation,
    reveal_progress: f32,
) -> bool {
    match presentation {
        Presentation::Reserved => true,
        Presentation::Overlay => reveal_progress > 0.0,
        Presentation::Hidden => false,
    }
}

pub(super) struct TabItem {
    pub(super) id: TabId,
    pub(super) index: usize,
    pub(super) title: String,
    pub(super) exited: bool,
    pub(super) active: bool,
    pub(super) follows_active: bool,
}

/// Content shared by every tab style: status, title, and close button.
struct TabParts {
    status: Option<Svg>,
    title: Div,
    close: Stateful<Div>,
}

impl WorkspaceView {
    /// Renders one tab at `bounds`, relative to the tab strip.
    pub(super) fn tab_element(
        item: &TabItem,
        bounds: Bounds<Pixels>,
        position: TabPosition,
        style: TabStyle,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Stateful<Div> {
        let id = item.id;
        let shell = div()
            .id(("tab", id.get()))
            .group("tab")
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .flex_shrink_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .cursor_pointer()
            .text_color(if item.active {
                colors.foreground
            } else {
                colors.inactive
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.begin_reorder(id, event.position, window, cx);
                    cx.stop_propagation();
                }),
            );
        let parts = TabParts {
            status: item
                .exited
                .then(|| icon_element(Icon::CircleAlert, colors.error)),
            title: div()
                .flex_1()
                .min_w_0()
                .text_ellipsis()
                .when(item.exited, |title| title.opacity(0.6))
                .child(item.title.clone()),
            close: Self::close_tab_button(id, item.active, colors, cx),
        };
        if position.vertical() {
            vertical_tab(shell, parts, item.active, colors)
        } else {
            match style {
                TabStyle::Strip => {
                    strip_tab(shell, parts, item, position, colors)
                }
                TabStyle::Pill => pill_tab(shell, parts, item, colors),
            }
        }
    }

    /// Keeps its slot while hidden so hovering never changes tab width.
    fn close_tab_button(
        id: TabId,
        active: bool,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Stateful<Div> {
        let foreground = colors.foreground;
        div()
            .id(("close-tab", id.get()))
            .group("close-tab")
            .flex_shrink_0()
            .w(px(18.0))
            .h(px(18.0))
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_center()
            .when(!active, |button| {
                button
                    .opacity(0.0)
                    .group_hover("tab", |style| style.opacity(1.0))
            })
            .hover(|style| style.bg(foreground.opacity(0.08)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |view, _, window, cx| {
                cx.stop_propagation();
                view.request_close(CloseTarget::Tab(id), window, cx);
            }))
            .child(
                icon_element(Icon::X, colors.inactive)
                    .group_hover("close-tab", |style| {
                        style.text_color(foreground)
                    }),
            )
    }
}

/// Left and right placement share one row style regardless of `tabs.style`.
fn vertical_tab(
    shell: Stateful<Div>,
    parts: TabParts,
    active: bool,
    colors: TabColors,
) -> Stateful<Div> {
    let (hover, foreground) = (colors.hover(), colors.foreground);
    shell.child(
        div()
            .relative()
            .flex_1()
            .h_full()
            .mx(px(6.0))
            .my(px(1.0))
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap(px(9.0))
            .pl(px(14.0))
            .pr(px(6.0))
            .when(active, |row| {
                row.bg(colors.active).child(
                    div()
                        .absolute()
                        .left(px(5.0))
                        .top(px(8.0))
                        .bottom(px(8.0))
                        .w(px(3.0))
                        .rounded(px(3.0))
                        .bg(colors.accent),
                )
            })
            .when(!active, |row| {
                row.group_hover("tab", |style| {
                    style.bg(hover).text_color(foreground)
                })
            })
            .children(parts.status)
            .child(parts.title)
            .child(parts.close),
    )
}

/// The active tab merges into the terminal with an accent on its outer edge.
fn strip_tab(
    shell: Stateful<Div>,
    parts: TabParts,
    item: &TabItem,
    position: TabPosition,
    colors: TabColors,
) -> Stateful<Div> {
    let (hover, foreground) = (colors.hover(), colors.foreground);
    let separator = !item.active && item.index > 0 && !item.follows_active;
    shell
        .gap(px(7.0))
        .pl(px(12.0))
        .pr(px(6.0))
        .when(item.active, |tab| {
            tab.bg(colors.terminal).child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .h(px(2.0))
                    .bg(colors.accent)
                    .when(position == TabPosition::Top, Styled::top_0)
                    .when(position != TabPosition::Top, Styled::bottom_0),
            )
        })
        .when(!item.active, |tab| {
            tab.hover(|style| style.bg(hover).text_color(foreground))
        })
        .when(separator, |tab| {
            tab.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(9.0))
                    .bottom(px(9.0))
                    .w(px(1.0))
                    .bg(colors.border),
            )
        })
        .children(parts.status)
        .child(parts.title)
        .child(parts.close)
}

/// Rounded tabs with a numbered badge doubling as the shortcut hint.
fn pill_tab(
    shell: Stateful<Div>,
    parts: TabParts,
    item: &TabItem,
    colors: TabColors,
) -> Stateful<Div> {
    let (hover, foreground) = (colors.hover(), colors.foreground);
    let active = item.active;
    let badge = (item.index < 9).then(|| {
        div()
            .w(px(16.0))
            .h(px(16.0))
            .flex_shrink_0()
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(10.5))
            .when(active, |badge| {
                badge
                    .bg(colors.accent)
                    .text_color(colors.bar)
                    .font_weight(FontWeight::BOLD)
            })
            .when(!active, |badge| badge.bg(foreground.opacity(0.06)))
            .child((item.index + 1).to_string())
    });
    shell.px(px(2.0)).child(
        div()
            .flex_1()
            .h(PILL_HEIGHT)
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .pl(px(6.0))
            .pr(px(5.0))
            .when(active, |pill| pill.bg(colors.active))
            .when(!active, |pill| {
                pill.group_hover("tab", |style| {
                    style.bg(hover).text_color(foreground)
                })
            })
            .children(badge)
            .children(parts.status)
            .child(parts.title)
            .child(parts.close),
    )
}

#[cfg(test)]
mod tests {
    use gpui::size;

    use super::super::ChromeLayout;
    use super::*;

    #[test]
    fn top_chrome_follows_a_visible_or_revealing_tab_bar() {
        assert!(top_chrome_uses_bar(Presentation::Reserved, 0.0));
        assert!(!top_chrome_uses_bar(Presentation::Hidden, 1.0));
        assert!(!top_chrome_uses_bar(Presentation::Overlay, 0.0));
        assert!(top_chrome_uses_bar(Presentation::Overlay, 0.01));
    }

    #[test]
    fn tab_border_sits_on_the_terminal_edge_for_every_placement() {
        let viewport = size(px(800.0), px(600.0));
        for position in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
        ] {
            let layout = ChromeLayout::new(viewport, px(28.0), position);
            let border = layout.tab_border(position);
            let terminal = layout.terminal;
            let touches = match position {
                TabPosition::Top => border.bottom() == terminal.origin.y,
                TabPosition::Bottom => border.origin.y == terminal.bottom(),
                TabPosition::Left => border.right() == terminal.origin.x,
                TabPosition::Right => border.origin.x == terminal.right(),
            };
            assert!(touches, "{position:?}: {border:?} vs {terminal:?}");
            assert!(
                layout.tabs.contains(&border.origin),
                "{position:?} border leaves the tab bar"
            );
        }
    }
}
