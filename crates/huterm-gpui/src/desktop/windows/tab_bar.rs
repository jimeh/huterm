//! Theme-derived tab bar colors and tab item presentation.

use gpui::{Div, FontWeight, Hsla, Stateful, Svg, TextRun, svg};
use huterm_config::{TabStyle, TabWidth};

use crate::assets::Icon;

use super::{
    App, Bounds, CloseTarget, Context, FluentBuilder, InteractiveElement,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Presentation,
    StatefulInteractiveElement, Styled, TabId, TabPosition, Theme, Window,
    WorkspaceView, color, div, px,
};

/// Bounds the title width cache so long-lived windows with changing titles
/// cannot grow it without limit.
const TITLE_WIDTH_CACHE_LIMIT: usize = 512;

const PILL_HEIGHT: Pixels = px(26.0);
const ICON_SIZE: Pixels = px(12.0);
const CLOSE_SIZE: Pixels = px(18.0);
const BADGE_SIZE: Pixels = px(16.0);
const STRIP_GAP: Pixels = px(7.0);
const STRIP_PADDING_LEFT: Pixels = px(12.0);
const STRIP_PADDING_RIGHT: Pixels = px(6.0);
const PILL_GAP: Pixels = px(6.0);
const PILL_PADDING_LEFT: Pixels = px(6.0);
const PILL_PADDING_RIGHT: Pixels = px(5.0);
const PILL_MARGIN: Pixels = px(2.0);
/// Title text size used both to render and to measure Fit tabs.
pub(super) const TAB_TEXT_SIZE: Pixels = px(13.0);

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
        .w(ICON_SIZE)
        .h(ICON_SIZE)
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

    /// Caches Fit tab widths for `tab_strip`, which cannot read titles
    /// because it has no `App`.
    pub(super) fn measure_tab_widths(&mut self, window: &Window, cx: &App) {
        let tabs = self.config.tabs;
        if tabs.width != TabWidth::Fit || tabs.position.vertical() {
            self.tab_widths.clear();
            return;
        }
        if self.title_widths.len() > TITLE_WIDTH_CACHE_LIMIT {
            self.title_widths.clear();
        }
        let font = window.text_style().font();
        let mut widths = Vec::with_capacity(self.tabs.len());
        for (index, tab) in self.tabs.iter().enumerate() {
            let (title, exited) = tab.label(cx);
            let text = if let Some(width) = self.title_widths.get(&title) {
                *width
            } else {
                let run = TextRun {
                    len: title.len(),
                    font: font.clone(),
                    color: Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let width = window
                    .text_system()
                    .layout_line(&title, TAB_TEXT_SIZE, &[run], None)
                    .width;
                self.title_widths.insert(title, width);
                width
            };
            widths.push(fit_tab_width(
                text,
                tabs.style,
                exited,
                index,
                tabs.min_width,
                tabs.max_width,
            ));
        }
        self.tab_widths = widths;
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
            .w(CLOSE_SIZE)
            .h(CLOSE_SIZE)
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
        .gap(STRIP_GAP)
        .pl(STRIP_PADDING_LEFT)
        .pr(STRIP_PADDING_RIGHT)
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
            .w(BADGE_SIZE)
            .h(BADGE_SIZE)
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
    shell.px(PILL_MARGIN).child(
        div()
            .flex_1()
            .h(PILL_HEIGHT)
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap(PILL_GAP)
            .pl(PILL_PADDING_LEFT)
            .pr(PILL_PADDING_RIGHT)
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

/// The width a horizontal Fit tab needs around `title_width` of text, clamped
/// to the configured bounds. It mirrors the padding, gaps, and slots rendered
/// by `strip_tab` and `pill_tab`.
pub(super) fn fit_tab_width(
    title_width: Pixels,
    style: TabStyle,
    exited: bool,
    index: usize,
    min_width: f32,
    max_width: f32,
) -> Pixels {
    let status = if exited { ICON_SIZE } else { px(0.0) };
    // Slots are the flex children: title and close, plus status and badge.
    let chrome = match style {
        TabStyle::Strip => {
            let slots = 2 + u8::from(exited);
            STRIP_PADDING_LEFT
                + STRIP_PADDING_RIGHT
                + STRIP_GAP * f32::from(slots - 1)
                + CLOSE_SIZE
                + status
        }
        TabStyle::Pill => {
            let badge = index < 9;
            let slots = 2 + u8::from(exited) + u8::from(badge);
            PILL_MARGIN * 2.0
                + PILL_PADDING_LEFT
                + PILL_PADDING_RIGHT
                + PILL_GAP * f32::from(slots - 1)
                + CLOSE_SIZE
                + status
                + if badge { BADGE_SIZE } else { px(0.0) }
        }
    };
    (title_width + chrome)
        .ceil()
        .clamp(px(min_width), px(max_width))
}

#[cfg(test)]
mod tests {
    use gpui::size;

    use super::super::ChromeLayout;
    use super::*;

    #[test]
    fn fit_widths_add_style_chrome_and_clamp_to_bounds() {
        let strip =
            fit_tab_width(px(20.0), TabStyle::Strip, false, 0, 48.0, 600.0);
        assert_eq!(strip, px(20.0 + 12.0 + 6.0 + 7.0 + 18.0));
        assert_eq!(
            fit_tab_width(px(20.0), TabStyle::Strip, true, 0, 48.0, 600.0),
            strip + ICON_SIZE + STRIP_GAP
        );
        let badged =
            fit_tab_width(px(20.0), TabStyle::Pill, false, 8, 48.0, 600.0);
        let unbadged =
            fit_tab_width(px(20.0), TabStyle::Pill, false, 9, 48.0, 600.0);
        assert_eq!(badged - unbadged, BADGE_SIZE + PILL_GAP);
        assert_eq!(
            fit_tab_width(px(20.4), TabStyle::Strip, false, 0, 48.0, 600.0),
            px(64.0)
        );
        assert_eq!(
            fit_tab_width(px(1.0), TabStyle::Strip, false, 0, 96.0, 240.0),
            px(96.0)
        );
        assert_eq!(
            fit_tab_width(px(900.0), TabStyle::Pill, false, 0, 96.0, 240.0),
            px(240.0)
        );
    }

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
